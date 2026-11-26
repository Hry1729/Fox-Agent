import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { checkSelectedModelRoute, FoxTurnOutcome, modelFailure } from "./fox-pi-turn-outcome.mjs";

const [cli, gate] = process.argv.slice(2);
const agentDir = join(homedir(), ".pi", "agent");
const sessionDir = join(agentDir, "fox-sessions");
const send = (message) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n");
const textBlock = (text) => ({ type: "text", text });
let pi, sessionId, turn;
let sequence = 0;
const requests = new Map();
const dialogs = new Map();

function readFrames(stream, handle) {
	let tail = "";
	stream.setEncoding("utf8");
	stream.on("data", (data) => {
		tail += data;
		let end;
		while ((end = tail.indexOf("\n")) >= 0) {
			const line = tail.slice(0, end);
			tail = tail.slice(end + 1);
			if (!line.trim()) continue;
			handle(JSON.parse(line));
		}
	});
}

function sendPi(frame) {
	if (!pi?.stdin.writable) throw new Error("Pi RPC connection closed");
	pi.stdin.write(JSON.stringify(frame) + "\n");
}

function rpc(type, fields = {}) {
	const id = ++sequence;
	return new Promise((resolve, reject) => {
		requests.set(id, { resolve, reject });
		try {
			sendPi({ id, type, ...fields });
		} catch (error) {
			requests.delete(id);
			reject(error);
		}
	});
}

function update(sessionUpdate, fields) {
	send({ method: "session/update", params: { sessionId, update: { sessionUpdate, ...fields } } });
}

function reportCompaction(reason, result) {
	update("tool_call", {
		toolCallId: "pi-compaction-" + randomUUID(), title: "压缩上下文", kind: "think", status: "completed",
		_meta: { fox: { piCompaction: { reason, tokensBefore: result.tokensBefore } } },
	});
}

function messageText(content) {
	return content.filter((block) => block.type === "text").map((block) => block.text).join("");
}

function failTurn(error) {
	if (!turn) return;
	for (const steering of turn.steering) steering.reject(error);
	turn.reject(error);
	turn = undefined;
}

function closePi() {
	if (pi?.stdin.writable) pi.stdin.end();
}

function onPi(frame) {
	if (frame.type === "response") {
		const pending = requests.get(frame.id);
		if (!pending) return;
		requests.delete(frame.id);
		if (frame.success) pending.resolve(frame.data);
		else pending.reject(new Error(frame.error));
		return;
	}
	if (!turn) return;
	turn.outcome.observe(frame);
	switch (frame.type) {
		case "compaction_end":
		case "auto_compaction_end": {
			if (!turn.compacting && !frame.aborted && !frame.errorMessage && frame.result) {
				reportCompaction(frame.reason, frame.result);
			}
			break;
		}
		case "message_start": {
			if (frame.message.role !== "user") break;
			const content = messageText(frame.message.content);
			const index = turn.steering.findIndex((item) => item.text === content);
			if (index >= 0) turn.steering.splice(index, 1)[0].resolve({ outcome: "injected" });
			break;
		}
		case "message_update": {
			const event = frame.assistantMessageEvent;
			if (event.type === "text_delta" || event.type === "thinking_delta") {
				update(event.type === "text_delta" ? "agent_message_chunk" : "agent_thought_chunk",
					{ content: textBlock(event.delta) });
			}
			break;
		}
		case "tool_execution_start": {
			const { toolCallId, toolName, args } = frame;
			const kinds = { read: "read", write: "edit", edit: "edit", bash: "execute", powershell: "execute",
				grep: "search", find: "search", ls: "search" };
			update("tool_call", { toolCallId, title: toolName, kind: kinds[toolName] ?? "other",
				status: "in_progress", rawInput: args, locations: typeof args?.path === "string" ? [{ path: args.path }] : [] });
			break;
		}
		case "tool_execution_update":
		case "tool_execution_end": {
			const result = frame.result ?? frame.partialResult;
			const status = frame.type === "tool_execution_update" ? "in_progress" : frame.isError ? "failed" : "completed";
			const text = messageText(result?.content ?? []);
			const exitCode = result?.structuredContent?.exit_code;
			const terminalExit = frame.type === "tool_execution_end"
				&& (frame.toolName === "bash" || frame.toolName === "powershell")
				&& Number.isInteger(exitCode);
			update("tool_call_update", { toolCallId: frame.toolCallId, status,
				...(terminalExit ? { _meta: { terminal_exit: { exit_code: exitCode } } } : {}),
				...(text ? { content: [{ type: "content", content: textBlock(text) }] } : {}) });
			break;
		}
		case "extension_ui_request": {
			const select = frame.method === "select" && frame.options?.length > 0;
			if (frame.method !== "confirm" && !select) {
				if (["select", "input", "editor"].includes(frame.method)) {
					sendPi({ id: frame.id, type: "extension_ui_response", cancelled: true });
				}
				break;
			}
			const id = "pi-permission-" + randomUUID();
			const options = select
				? frame.options.map((name, index) => ({ optionId: "choice-" + index, name, kind: "allow_once" }))
				: [{ optionId: "yes", name: "允许", kind: "allow_once" }, { optionId: "no", name: "拒绝", kind: "reject_once" }];
			dialogs.set(id, { id: frame.id, options: select ? options : undefined });
			send({ id, method: "session/request_permission", params: {
				sessionId,
				toolCall: { toolCallId: "pi-ui-" + frame.id, title: frame.title, kind: "other",
					status: "pending", rawInput: { method: frame.method, title: frame.title, message: frame.message } },
				options,
			} });
			break;
		}
		case "agent_settled": {
			if (turn.compacting) break;
			if (!turn.cancelled && turn.outcome.failure) {
				const data = turn.outcome.failure;
				failTurn(Object.assign(new Error(data.message), { data }));
				break;
			}
			for (const steering of turn.steering) steering.reject(new Error("Pi 原回合已结束。"));
			turn.resolve({ stopReason: turn.cancelled ? "cancelled" : "end_turn" });
			turn = undefined;
			break;
		}
	}
}

function sessionRecord(id) {
	if (!/^[a-z0-9-]+$/i.test(id)) throw new Error("Invalid Pi session ID");
	return join(sessionDir, id + ".json");
}

function loadSession(id, cwd) {
	const own = sessionRecord(id);
	const entry = existsSync(own)
		? JSON.parse(readFileSync(own, "utf8"))
		: JSON.parse(readFileSync(join(dirname(agentDir), "pi-acp", "session-map.json"), "utf8")).sessions[id];
	if (!entry || resolve(entry.cwd) !== resolve(cwd) || !existsSync(entry.sessionFile)) {
		throw new Error("Pi session could not be restored");
	}
	return entry.sessionFile;
}

async function open(cwd, previousId) {
	const file = previousId ? loadSession(previousId, cwd) : undefined;
	const args = [cli, "--mode", "rpc", "--no-themes", "-e", gate];
	if (process.env.FOX_PI_MCP_EXTENSION) args.push("-e", process.env.FOX_PI_MCP_EXTENSION);
	if (file) args.push("--session", file);
	const env = { ...process.env, PI_CODING_AGENT_DIR: agentDir };
	const child = spawn(process.execPath, args, { cwd, env, stdio: ["pipe", "pipe", "inherit"], windowsHide: true });
	pi = child;
	const disconnected = () => {
		if (pi !== child) return;
		const error = new Error("Pi RPC process exited");
		for (const pending of requests.values()) pending.reject(error);
		requests.clear();
		dialogs.clear();
		failTurn(error);
		pi = undefined;
	};
	child.once("error", disconnected);
	child.once("close", disconnected);
	child.stdin.on("error", disconnected);
	readFrames(child.stdout, onPi);
	const state = await rpc("get_state");
	sessionId = previousId ?? state.sessionId;
	mkdirSync(sessionDir, { recursive: true });
	writeFileSync(sessionRecord(sessionId), JSON.stringify({ cwd, sessionFile: state.sessionFile }));
	return { sessionId };
}

async function startPrompt(blocks) {
	if (turn) throw new Error("Pi is already running");
	const message = messageText(blocks);
	const compacting = message.trim() === "/compact" && blocks.every((block) => block.type === "text");
	const done = Promise.withResolvers();
	const active = { ...done, cancelled: false, compacting, steering: [], outcome: new FoxTurnOutcome() };
	turn = active;
	try {
		const images = blocks.filter((block) => block.type === "image")
			.map(({ data, mimeType }) => ({ type: "image", data, mimeType }));
		const [, result] = await Promise.all([
			checkSelectedModelRoute({ getState: () => rpc("get_state") }).catch((error) => {
				const data = modelFailure(error);
				throw Object.assign(new Error(data.message), { data });
			}).then(async () => {
				if (compacting) {
					const result = await rpc("compact");
					if (!active.cancelled) reportCompaction("manual", result);
					active.resolve({ stopReason: active.cancelled ? "cancelled" : "end_turn" });
					if (turn === active) turn = undefined;
					return;
				}
				const ack = await rpc("prompt", { message, images });
				if (ack.disposition === "handled") {
					turn?.resolve({ stopReason: "end_turn" });
					turn = undefined;
				} else if (ack.disposition !== "started") {
					throw new Error("Pi did not start the requested turn");
				}
			}),
			done.promise,
		]);
		return result;
	} catch (error) {
		failTurn(error);
		throw error;
	}
}

async function steer(blocks) {
	const active = turn;
	if (!active || active.cancelled || active.compacting) throw new Error("Pi 当前没有可追加指令的回合。");
	const pending = { ...Promise.withResolvers(), text: messageText(blocks) };
	active.steering.push(pending);
	try {
		const [, received] = await Promise.all([
			rpc("steer", { message: pending.text }).then((ack) => {
				if (ack.disposition !== "queued") throw new Error("Pi 未将补充指令排入当前回合。");
			}),
			pending.promise,
		]);
		return received;
	} finally {
		active.steering = active.steering.filter((item) => item !== pending);
	}
}

async function handle(method, params) {
	if (method === "initialize") {
		const version = JSON.parse(readFileSync(join(dirname(cli), "..", "..", "package.json"), "utf8")).version;
		return { protocolVersion: 1, agentInfo: { name: "fox-pi-rpc", version },
			agentCapabilities: { loadSession: true, promptCapabilities: { image: true } },
			_meta: { steering: { supported: true } } };
	}
	if (method === "session/new") return open(params.cwd);
	if (method === "session/load") {
		await open(params.cwd, params.sessionId);
		return {};
	}
	if (params.sessionId !== sessionId || !pi) throw new Error("Unknown Pi session");
	switch (method) {
		case "session/prompt": return startPrompt(params.prompt);
		case "session/set_config_option": {
			const separator = typeof params.value === "string" ? params.value.indexOf("/") : -1;
			if (params.configId !== "model" || separator <= 0 || separator === params.value.length - 1) {
				throw Object.assign(new Error("Expected a Pi model in provider/model format"), { code: -32602 });
			}
			await rpc("set_model", { provider: params.value.slice(0, separator), modelId: params.value.slice(separator + 1) });
			return { configOptions: [] };
		}
		case "session/cancel":
			if (turn) {
				turn.cancelled = true;
				await rpc("abort");
			}
			return {};
		case "_session/steering": return steer(params.prompt);
		default: throw Object.assign(new Error("Method not found"), { code: -32601 });
	}
}

readFrames(process.stdin, (message) => {
	if (!message.method) {
		const dialog = dialogs.get(message.id);
		if (dialog === undefined) return;
		dialogs.delete(message.id);
		if (!pi) return;
		const selected = message.result?.outcome?.optionId;
		const choice = dialog.options?.find((option) => option.optionId === selected);
		const result = dialog.options
			? choice ? { value: choice.name } : { cancelled: true }
			: { confirmed: selected === "yes" };
		sendPi({ id: dialog.id, type: "extension_ui_response", ...result });
		return;
	}
	void handle(message.method, message.params ?? {}).then(
		(result) => { if (message.id !== undefined) send({ id: message.id, result }); },
		(error) => { if (message.id !== undefined) send({ id: message.id,
			error: { code: error.code ?? -32603, message: error.message, data: error.data } }); },
	);
});
process.stdin.on("end", closePi);
