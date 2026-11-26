"""Run selected frozen Fox cases through the installed Pi RPC and pi-acp binaries."""

import argparse
from contextlib import contextmanager
import csv
import json
import os
from pathlib import Path
from queue import Empty, Queue
import shutil
import socket
import subprocess
import sys
from threading import Thread
import time
from urllib.request import ProxyHandler, Request, build_opener


HERE = Path(__file__).resolve().parent
SERVER = "fox_stress"
NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)
HTTP = build_opener(ProxyHandler({}))
CASES = (
    ("S03", "default", "rpc"),
    ("S03", "default", "acp"),
    ("S08", "present", "rpc"),
    ("S08", "present", "acp"),
    ("S08", "present", "acp-extension"),
    ("S08", "absent", "rpc"),
    ("S08", "absent", "acp"),
    ("S09", "default", "rpc"),
    ("S09", "default", "acp"),
    ("S03", "default", "acp-extension"),
)


def save(path, data):
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def load(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def log(path, direction, message):
    with path.open("a", encoding="utf-8") as file:
        file.write(json.dumps({"at": time.time(), "direction": direction, "frame": message},
                              ensure_ascii=False) + "\n")


def stop(process):
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=5)


def mcp_tap(trace, command):
    """Tee MCP frames while running the original frozen MCP server."""
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          creationflags=NO_WINDOW) as child:
        def forward_input():
            try:
                for line in sys.stdin.buffer:
                    log(trace, "to-server", json.loads(line))
                    child.stdin.write(line)
                    child.stdin.flush()
            finally:
                child.stdin.close()

        Thread(target=forward_input, daemon=True).start()
        for line in child.stdout:
            log(trace, "from-server", json.loads(line))
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
        return child.wait()


class Agent:
    def __init__(self, command, cwd, env, out, route, generation):
        self.route = route
        self.cwd = cwd
        self.trace = out / f"agent-{generation}.jsonl"
        self.stdout = Queue()
        self.next_id = 0
        self.stderr = (out / f"agent-{generation}.stderr").open("wb")
        self.child = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=self.stderr, creationflags=NO_WINDOW)
        self.thread = Thread(target=self.read, daemon=True)
        self.thread.start()

    @property
    def native(self):
        return self.route == "rpc"

    def read(self):
        try:
            for line in self.child.stdout:
                frame = json.loads(line)
                log(self.trace, "from-agent", frame)
                self.stdout.put(frame)
        except (ValueError, OSError) as error:
            self.stdout.put(error)
        finally:
            self.stdout.put(EOFError("Pi process closed its protocol stream"))

    def send(self, frame):
        log(self.trace, "to-agent", frame)
        self.child.stdin.write((json.dumps(frame, ensure_ascii=False) + "\n").encode("utf-8"))
        self.child.stdin.flush()

    def request(self, method, params=None):
        self.next_id += 1
        body = params or {}
        if self.native:
            self.send({"id": self.next_id, "type": method, **body})
        else:
            self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": body})
        return self.next_id

    def receive(self, timeout):
        frame = self.stdout.get(timeout=timeout)
        if isinstance(frame, Exception):
            raise frame
        return frame

    def close(self):
        try:
            self.child.stdin.close()
            try:
                self.child.wait(timeout=4)
            except subprocess.TimeoutExpired:
                stop(self.child)
            self.thread.join(timeout=2)
        finally:
            self.stderr.close()

    def permission(self, frame):
        if self.native and frame.get("type") == "extension_ui_request" and frame.get("method") == "confirm":
            return json.loads(frame["message"]), {"type": "extension_ui_response", "id": frame["id"]}
        if frame.get("method") == "session/request_permission":
            tool = frame["params"]["toolCall"]
            return json.loads(tool["rawInput"]["message"]), {"jsonrpc": "2.0", "id": frame["id"]}
        return None

    def approve(self, request, reply):
        name = request["toolName"]
        data = request.get("input") or {}
        permitted = name in {
            f"mcp__{SERVER}__query_order", f"mcp__{SERVER}__append_order",
            f"mcp__{SERVER}__scan_config",
        }
        if name in {"read", "ls", "find", "grep", "write", "edit"}:
            dest = (self.cwd / data.get("path", "")).resolve()
            allowed_root = self.cwd / "output" if name in {"write", "edit"} else self.cwd
            permitted = dest.is_relative_to(allowed_root)
        if self.native:
            reply["confirmed"] = permitted
        else:
            reply["result"] = {"outcome": {"outcome": "selected", "optionId": "yes" if permitted else "no"}}
        self.send(reply)

    def response(self, request_id, timeout=40):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            frame = self.receive(max(0.01, deadline - time.monotonic()))
            if self.native:
                response = frame.get("type") == "response"
            else:
                response = "result" in frame or "error" in frame
            if response and frame.get("id") == request_id:
                return frame
            permit = self.permission(frame)
            if permit:
                self.approve(*permit)
        raise TimeoutError("No command response")

    def init(self, server, session=None):
        if self.native:
            return self.response(self.request("get_state"))
        self.response(self.request("initialize", {"protocolVersion": 1, "clientCapabilities": {},
                                                  "clientInfo": {"name": "fox-pi-comparison", "version": "1"}}))
        params = {"cwd": str(self.cwd), "mcpServers": [server] if server else []}
        if session:
            params["sessionId"] = session
        frame = self.response(self.request("session/load" if session else "session/new", params))
        if "error" in frame:
            raise RuntimeError(frame["error"]["message"])
        return session or frame["result"]["sessionId"]

    def prompt(self, message, session):
        if self.native:
            return self.request("prompt", {"message": message})
        return self.request("session/prompt", {"sessionId": session, "prompt": [{"type": "text", "text": message}]})


@contextmanager
def service(frozen, node, out, task, scenario, port):
    if task == "S09":
        yield None, None
        return
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", port))
    base = f"http://127.0.0.1:{port}"
    config = out / "service-config.json"
    save(config, {"host": "127.0.0.1", "port": port, "serviceUrl": base, "stateFile": "state.json"})
    source = frozen / "test-services"

    def call(path, body=None):
        headers = {"Content-Type": "application/json"}
        data = None if body is None else json.dumps(body).encode()
        with HTTP.open(Request(base + path, data=data, headers=headers), timeout=2) as response:
            return json.load(response)

    with (out / "service.log").open("wb") as log_file:
        process = subprocess.Popen([node, source / "stress-service.mjs", "--config", config],
                                   stdout=log_file, stderr=subprocess.STDOUT, creationflags=NO_WINDOW)
        try:
            for _ in range(50):
                try:
                    call("/health")
                    break
                except OSError:
                    if process.poll() is not None:
                        raise RuntimeError("Frozen stress service failed to start")
                    time.sleep(0.1)
            params = {"scenario": task}
            if task == "S08":
                params["subscenario"] = scenario
            call("/v1/admin/reset", params)
            server = {"name": SERVER, "command": sys.executable, "env": [],
                      "args": [str(path) for path in (Path(__file__).resolve(), "--tap-mcp", out / "mcp.jsonl",
                                                       node, source / "stress-mcp.mjs", "--config", config)]}
            evidence = lambda: call("/v1/evidence")
            try:
                yield server, evidence
            finally:
                save(out / "service-final.json", evidence())
        finally:
            stop(process)


def run_turn(agent, session, initial, followup, evidence, interrupt, deadline, result):
    prompt = agent.prompt(initial, session)
    reply_id = steer = None
    text = []
    while time.monotonic() < deadline:
        if interrupt:
            ledger = evidence()["state"]["domains"]["s03"]
            if any(row["responseDisposition"] == "withheld" for row in ledger["appendAttempts"]):
                result["withheldWriteObserved"] = True
                return True
        try:
            frame = agent.receive(0.15)
        except Empty:
            continue
        permit = agent.permission(frame)
        if permit:
            if followup and steer is None and permit[0]["toolName"] == "read":
                result["steeringTrigger"] = permit[0]
                if agent.native:
                    steer = agent.request("steer", {"message": followup})
                else:
                    steer = agent.request("_session/steering", {"sessionId": session, "text": followup})
                    reply_id = agent.prompt(followup, session)
            agent.approve(*permit)
        is_response = frame.get("type") == "response" if agent.native else ("result" in frame or "error" in frame)
        if is_response and steer is not None and frame.get("id") == steer:
            result["steeringReply"] = frame
        if agent.native:
            if frame.get("type") == "message_end":
                message = frame["message"]
                if message["role"] == "assistant":
                    text.append("".join(block["text"] for block in message["content"] if block["type"] == "text"))
                    result["model"] = {key: message.get(key) for key in ("model", "provider", "stopReason")}
                    if message.get("errorMessage"):
                        result["modelError"] = message["errorMessage"]
                if followup and message.get("role") == "user":
                    blocks = message.get("content", [])
                    if isinstance(blocks, list) and any(followup == block.get("text") for block in blocks):
                        result["steeringDelivered"] = True
            if frame.get("type") == "agent_settled":
                result["terminal"] = frame["type"]
                break
            if frame.get("id") == prompt and frame.get("success") is False:
                raise RuntimeError(frame.get("error"))
        else:
            update = frame.get("params", {}).get("update", {})
            if update.get("sessionUpdate") == "agent_message_chunk":
                text.append(update.get("content", {}).get("text", ""))
            if is_response and frame.get("id") in {prompt, reply_id}:
                result.setdefault("turnReplies", []).append(frame)
                if "error" in frame:
                    result["error"] = frame["error"]["message"]
                if frame["id"] == (reply_id or prompt):
                    result["terminal"] = "session/prompt response"
                    break
    else:
        result["error"] = "Turn timeout"
    result["assistantText"] = "".join(text)
    return False


def run_case(args, case, port):
    task, scenario, route = case
    out = args.output / f"{task}-{scenario}-{route}"
    out.mkdir()
    work = out / "work"
    work.mkdir()
    (work / "output").mkdir()
    profile = out / "home" / ".pi" / "agent"
    profile.mkdir(parents=True)
    runtime = args.frozen / "external-runtime"
    modules = runtime / "backends" / "pi" / "node_modules"
    node = runtime / "node" / "node.exe"
    gate = runtime / "pi-gate"
    pi = modules / "@earendil-works" / "pi-coding-agent" / "dist" / "bundle" / "cli.js"
    acp = modules / "pi-acp" / "dist" / "index.js"
    result = {"task": task, "scenario": scenario, "route": route, "modelRequested": args.model}
    settings = {"defaultProvider": "fox-comparison", "defaultModel": args.model,
                "defaultTools": ["-bash", "+ls", "+find", "+grep"]}
    save(profile / "settings.json", settings)
    save(profile / "models.json", {"providers": {"fox-comparison": {
        "api": "openai-completions", "baseUrl": args.base_url, "apiKey": "$FOX_PI_COMPARISON_KEY",
        "models": [{"id": args.model, "name": args.model, "reasoning": False, "input": ["text"],
                    "contextWindow": 262144, "maxTokens": 8192}],
    }}})
    stress_spec = args.frozen / "stress-specs" / task / "frozen-spec.json"
    spec = load(stress_spec)
    if task == "S09":
        source = args.frozen / spec["scenarios"][scenario]["materials"]
        shutil.copytree(source / "input", work / "input")
    env = os.environ.copy()
    for key in ("NODE_OPTIONS", "FOX_PI_MCP_EXTENSION", "FOX_PI_EXTRA_ARGS", "FOX_PI_TEST_EXTENSION"):
        env.pop(key, None)
    env.update({"HOME": str(profile.parent.parent), "USERPROFILE": str(profile.parent.parent),
                "PI_CODING_AGENT_DIR": str(profile),
                "PATH": os.pathsep.join(map(str, (node.parent, modules / ".bin", env.get("PATH", "")))),
                "PI_SKIP_VERSION_CHECK": "1", "PI_TELEMETRY_DISABLED": "1",
                "PI_ACP_PI_COMMAND": str(gate / "pi-gated.cmd"), "FOX_PI_BIN": str(modules / ".bin" / "pi.cmd")})

    with service(args.frozen, node, out, task, scenario, port) as (server, evidence):
        if server:
            env["FOX_PI_EXTRA_ARGS"] = "-e builtin:mcp"
        if server and route in {"rpc", "acp-extension"}:
            env["FOX_PI_COMPARISON_MCP"] = json.dumps(server)
            env["FOX_PI_TEST_EXTENSION"] = str(HERE / "native-rpc-mcp.ts")
        native_command = [node, pi, "--mode", "rpc", "--no-themes", "--no-extensions",
                          "-e", gate / "fox-approval-gate.ts"]
        if server:
            native_command += ["-e", "builtin:mcp", "-e", HERE / "native-rpc-mcp.ts"]
        command = native_command if route == "rpc" else [node, acp]
        try:
            agent = Agent(command, work, env, out, route, 1)
            write_interrupted = False
            try:
                session = agent.init(server)
                followup = spec["fixed_followups"][0] if task == "S09" else None
                write_interrupted = run_turn(agent, session, spec["initial_prompt"], followup,
                                             evidence, task == "S03", time.monotonic() + args.timeout, result)
                if write_interrupted:
                    save(out / "service-before-restart.json", evidence())
                    if agent.native:
                        state = agent.response(agent.request("get_state"))
                        session_file = state["data"]["sessionFile"]
                        command = [*command, "--session", session_file]
                    stop(agent.child)
            finally:
                agent.close()
            if write_interrupted:
                result["agentProcessRestarted"] = True
                agent = Agent(command, work, env, out, route, 2)
                try:
                    session = agent.init(server, session)
                    run_turn(agent, session, spec["fixed_followups"][0], None, evidence,
                             False, time.monotonic() + args.timeout, result)
                finally:
                    agent.close()
        except (OSError, RuntimeError, EOFError, Empty, KeyError) as error:
            result["error"] = f"{type(error).__name__}: {error}"
    outputs = {}
    for path in (work / "output").glob("*.csv"):
        with path.open(encoding="utf-8-sig", newline="") as file:
            outputs[path.name] = list(csv.reader(file))
    result["csv"] = outputs
    save(out / "result.json", result)
    status = result.get("error") or result.get("modelError") or result.get("terminal")
    print(f"{out.name}: {status or 'interrupted'}", flush=True)
    return result


def main():
    if sys.argv[1:2] == ["--tap-mcp"]:
        return mcp_tap(Path(sys.argv[2]), sys.argv[3:])
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--frozen", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--base-url", default="https://token.sensenova.cn/v1")
    parser.add_argument("--model", default="sensenova-6.8-flash")
    parser.add_argument("--timeout", type=float, default=105)
    parser.add_argument("--only", nargs="+", help="Case keys, for example S09-default-rpc")
    args = parser.parse_args()
    if not os.environ.get("FOX_PI_COMPARISON_KEY"):
        parser.error("FOX_PI_COMPARISON_KEY must be set by the model credential provider")
    selected = [(index, case) for index, case in enumerate(CASES)
                if args.only is None or "-".join(case) in args.only]
    if not selected or (args.only and len(selected) != len(set(args.only))):
        parser.error("Unknown case key")
    args.frozen = args.frozen.resolve(strict=True)
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    results = [run_case(args, case, 18779 + index) for index, case in selected]
    save(args.output / "summary.json", results)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
