"""Audited, version-pinned runtime adaptations. Never edits an installed runtime.

Only a staging directory is accepted by the package builder. Unknown source
hashes fail closed, including a partially patched or modified upstream bundle.
Re-running on an already patched runtime is a no-op: a file whose content
already equals its build-time pin (runtime-assets/model-adapters/pins.json)
is left as it is. Every patch anchor must match exactly once, or nothing is
written and the script exits non-zero.
"""
import hashlib
import json
import os
from pathlib import Path

CRATE = Path(__file__).resolve().parents[2]
ASSETS = CRATE / "runtime-assets" / "model-adapters"


def replace_once(source, old, new):
    if source.count(old) != 1:
        raise ValueError("pinned runtime patch anchor differs")
    return source.replace(old, new, 1)


def patch_pi_acp(source):
    source = replace_once(source, '#!/usr/bin/env node\n\n// src/index.ts', '#!/usr/bin/env node\n\nimport { FoxTurnOutcome, modelFailure, probeModelRoute, checkSelectedModelRoute } from "./fox-turn-outcome.mjs";\n\n// src/index.ts')
    source = replace_once(source, '  cancelRequested = false;', '  foxTurnOutcome = new FoxTurnOutcome();\n  foxModelProbe = null;\n  foxSettling = null;\n  cancelRequested = false;')
    source = replace_once(source, '  async settleTurn() {\n    await this.publishContextUsage();\n    const reason = this.cancelRequested ? "cancelled" : "end_turn";\n    this.pendingTurn?.resolve(reason);', '''  async settleTurn() {
    const turn = this.pendingTurn;
    if (!turn || this.foxSettling === turn) return;
    this.foxSettling = turn;
    await this.publishContextUsage();
    if (this.pendingTurn !== turn) return;
    const failure = this.foxTurnOutcome.failure;
    if (this.cancelRequested) turn.resolve("cancelled");
    else if (failure) turn.reject(RequestError2.internalError(failure, failure.message));
    else turn.resolve("end_turn");''')
    source = replace_once(source, '  startTurn(t) {\n    this.cancelRequested = false;', '  startTurn(t) {\n    this.foxTurnOutcome = new FoxTurnOutcome();\n    this.cancelRequested = false;')
    source = replace_once(source, '    this.proc.prompt(t.message, t.images).catch((err) => {', '''    const foxCurrentTurn = this.pendingTurn;
    checkSelectedModelRoute(this.proc).then(() => {
      this.foxModelProbe ??= probeModelRoute().catch(error => { this.foxModelProbe = null; throw error; });
      return this.foxModelProbe;
    }).then(() => this.proc.prompt(t.message, t.images)).catch((err) => {
      if (this.pendingTurn !== foxCurrentTurn) return;''')
    source = replace_once(source, '          const reason = this.cancelRequested ? "cancelled" : "error";\n          this.pendingTurn?.resolve(reason);', '''          if (this.cancelRequested) this.pendingTurn?.resolve("cancelled");
          else {
            const failure = modelFailure(err);
            this.pendingTurn?.reject(RequestError2.internalError(failure, failure.message));
          }''')
    source = replace_once(source, '  handlePiEvent(ev) {\n    const type', '  handlePiEvent(ev) {\n    if (this.pendingTurn) this.foxTurnOutcome.observe(ev);\n    const type')
    # Status updates remain visible diagnostics and cannot masquerade as an answer.
    for expression in ['this.startupInfo', 'formatAutoRetryMessage(ev)', '"Retry finished, resuming."']:
        anchor = f'      content: {{ type: "text", text: {expression} }}' if expression == 'this.startupInfo' else f'          content: {{ type: "text", text: {expression} }}'
        indentation = '      ' if expression == 'this.startupInfo' else '          '
        source = replace_once(source, anchor, indentation + '_meta: { fox: { kind: "backend_status" } },\n' + anchor)
    for text in ['Context nearing limit, running automatic compaction...', 'Automatic compaction finished; context was summarized to continue the session.']:
        source = replace_once(source, f'          content: {{\n            type: "text",\n            text: "{text}"', f'          _meta: {{ fox: {{ kind: "backend_status" }} }},\n          content: {{\n            type: "text",\n            text: "{text}"')
    return source


# fox-fix-console: Windows console windows. Fox starts the backend with
# CREATE_NO_WINDOW, so the tree shares one hidden console; windowsHide on the
# Node side keeps a child hidden even where it would get a console of its own
# (and is the Node equivalent of the Host's console::HideConsole).
PI_ACP_HIDDEN_SPAWNS = [
    # PiRpcProcess.spawn: pi-acp -> pi-gated.cmd (cross-spawn, cmd.exe) -> pi.
    ('      stdio: "pipe",\n      env: process.env\n    });', '      stdio: "pipe",\n      env: process.env,\n      windowsHide: true\n    });'),
    # /changelog lookup.
    ('spawnSync(whichCmd, ["pi"], { encoding: "utf-8" })', 'spawnSync(whichCmd, ["pi"], { encoding: "utf-8", windowsHide: true })'),
    ('spawnSync("npm", ["root", "-g"], { encoding: "utf-8" })', 'spawnSync("npm", ["root", "-g"], { encoding: "utf-8", windowsHide: true })'),
    # Update notice: npm view.
    ('      encoding: "utf-8",\n      timeout: 800\n    });', '      encoding: "utf-8",\n      timeout: 800,\n      windowsHide: true\n    });'),
]


def patch_pi_acp_hidden(source):
    for old, new in PI_ACP_HIDDEN_SPAWNS:
        source = replace_once(source, old, new)
    # Startup info and update notice both run `pi --version` (two call sites).
    old = 'spawnSync("pi", ["--version"], { encoding: "utf-8" })'
    if source.count(old) != 2:
        raise ValueError("pinned runtime patch anchor differs")
    source = source.replace(old, 'spawnSync("pi", ["--version"], { encoding: "utf-8", windowsHide: true })')
    # Left as is: `--terminal-login` (spawnSync2 with stdio "inherit") is the
    # interactive login a user starts in a terminal; Fox never passes it.
    return source


def patch_pi_acp_all(source):
    return patch_pi_acp_hidden(patch_pi_acp(source))


# pi 1.4.0 bundled CLI (dist/bundle/chunks/chunk-MM3LKT73.js, minified): the
# spawns an RPC session can reach. The bash tool (spawn2) and the clipboard
# helper (spawn5) already pass windowsHide; TUI-only spawns (editor, tmux, gh,
# trash) and the SDK's RpcClient are not reached through pi-acp.
PI_BUNDLE_HIDDEN_SPAWNS = [
    # core/exec.js: pi.exec() for extensions.
    'spawn(command,args,{cwd,shell:!1,stdio:["ignore","pipe","pipe"]',
    # tools-manager: `<fd|rg> --version` probe and archive extraction.
    'spawnSync2(cmd,["--version"],{stdio:"pipe"',
    'spawnSync2(command,args,{stdio:"pipe"',
    # find / grep tools.
    'spawn3(fdPath,args,{stdio:["ignore","pipe","pipe"]',
    'spawn4(rgPath,args,{stdio:["ignore","pipe","pipe"]',
    # git branch lookup (footer data, sync and async).
    'spawnSync3("git",["--no-optional-locks","symbolic-ref","--quiet","--short","HEAD"],{cwd:repoDir,encoding:"utf8",stdio:["ignore","pipe","ignore"]',
    'execFile("git",["--no-optional-locks","symbolic-ref","--quiet","--short","HEAD"],{cwd:repoDir,encoding:"utf8"',
]


def patch_pi_bundle_hidden(source):
    for anchor in PI_BUNDLE_HIDDEN_SPAWNS:
        source = replace_once(source, anchor + '}', anchor + ',windowsHide:!0}')
    return source


def patch_pi_bundle_all(source):
    return patch_pi_bundle_hidden(patch_pi_dispatcher(source))


def patch_dsh_proxy(source):
    source = 'import { foxModelProxyTarget } from "./fox-model-proxy.mjs";\n' + source
    return replace_once(source, '\tif (isLoopbackHost(url.hostname)) return void 0;', '\tif (isLoopbackHost(url.hostname) && foxModelProxyTarget(url)) return proxy;\n\tif (isLoopbackHost(url.hostname)) return void 0;')


def patch_pi_dispatcher(source):
    return replace_once(source, 'new undici.EnvHttpProxyAgent({', 'new undici.EnvHttpProxyAgent({\n        ...(process.env.FOX_MODEL_PROXY_REQUIRED === "1" ? { noProxy: "" } : {}),')


# Source hashes are npm's published JavaScript, LF-normalized. Versions are
# checked separately. Hashes are not inferred from whatever happens to be installed.
PATCHES = [
    {"id": "pi-acp-errors", "package": "backends/pi/node_modules/pi-acp", "version": "0.0.34", "file": "dist/index.js", "sha256": "324aeb8bba1228937e16b1326fb3e014e2a625b2dea549dbcd49b006cd5df6a2", "patch": patch_pi_acp_all, "helper": ("pi-turn-outcome.mjs", "dist/fox-turn-outcome.mjs")},
    {"id": "dsh-model-proxy", "package": "backends/dsh/node_modules/@deepseek-ai/dsh-http-proxy", "version": "0.2.1-rc.2", "file": "lib/index.js", "sha256": "5310861bc89788ea24be82a3b323e34cbd42b9e6214e2a179b2e6f92a8758cd5", "patch": patch_dsh_proxy, "helper": ("dsh-model-proxy.mjs", "lib/fox-model-proxy.mjs")},
    {"id": "pi-model-proxy", "package": "backends/pi/node_modules/@earendil-works/pi-coding-agent", "version": "1.4.0", "file": "dist/core/http-dispatcher.js", "sha256": "f9aa2c81b0a5958ffba6368506f24c9b202904c234ada19494cdc9c4a1d0e97b", "patch": patch_pi_dispatcher},
    {"id": "pi-bundled-model-proxy", "package": "backends/pi/node_modules/@earendil-works/pi-coding-agent", "version": "1.4.0", "file": "dist/bundle/chunks/chunk-MM3LKT73.js", "sha256": "aa4c8b091657bda611f6fd58343561fa44cda1070ae4cd5dbd09ffe1d9a34aab", "patch": patch_pi_bundle_all},
]


def digest(data): return hashlib.sha256(data).hexdigest()


def build_time_pins():
    return json.loads((ASSETS / 'pins.json').read_text(encoding='utf-8'))['files']


def apply(runtime):
    root = Path(runtime)
    pins = build_time_pins()
    manifest = {"revision": 1, "files": {}}
    # Check and compute everything first, write afterwards: a failing anchor
    # leaves the runtime untouched.
    writes = []
    for item in PATCHES:
        package = root / item['package']
        if not package.exists(): continue
        version = json.loads((package / 'package.json').read_text(encoding='utf-8'))['version']
        if version != item['version']: raise ValueError(f"{item['id']}: unvalidated version {version}")
        path = package / item['file']
        key = path.relative_to(root).as_posix()
        current = path.read_bytes().replace(b'\r\n', b'\n')
        if digest(current) == pins.get(key):
            patched = current  # already patched by this script: idempotent re-run
        elif digest(current) == item['sha256']:
            patched = item['patch'](current.decode('utf-8')).encode('utf-8')
            if digest(patched) != pins.get(key):
                raise ValueError(f"{item['id']}: patched result does not match its build-time pin")
        else:
            raise ValueError(f"{item['id']}: upstream source hash differs")
        writes.append((path, patched))
        manifest['files'][key] = digest(patched)
        if 'helper' in item:
            source, target = item['helper']
            data = (ASSETS / source).read_bytes().replace(b'\r\n', b'\n')
            helper = package / target
            writes.append((helper, data))
            manifest['files'][helper.relative_to(root).as_posix()] = digest(data)
    for path, data in writes:
        if not path.exists() or path.read_bytes() != data:
            path.write_bytes(data)
    (root / 'model-adapters.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    return manifest


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('runtime')
    args = parser.parse_args()
    try:
        print(json.dumps(apply(args.runtime), indent=2))
    except ValueError as error:
        raise SystemExit(f"runtime_patches: {error}")
