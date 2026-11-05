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
    return patch_pi_bundle_console(patch_pi_bundle_hidden(patch_pi_dispatcher(source)))


def patch_pi_shell_console(source):
    source = 'import { pipeFoxConsoleOutput } from "./fox-console-output.mjs";\n' + source
    source = replace_once(source, '                child.stdout?.on("data", onData);\n                child.stderr?.on("data", onData);',
                          '                const foxStdout = pipeFoxConsoleOutput(child.stdout, onData);\n                const foxStderr = pipeFoxConsoleOutput(child.stderr, onData);')
    return replace_once(source, 'const exitCode = await waitForChildProcess(child);',
                        'const exitCode = await waitForChildProcess(child);\n                foxStdout();\n                foxStderr();')


def patch_pi_bundle_console(source):
    source = 'import { pipeFoxConsoleOutput } from "./fox-console-output.mjs";\n' + source
    source = replace_once(source, ',child.stdout?.on("data",onData),child.stderr?.on("data",onData),signal&&',
                          ';let foxStdout=pipeFoxConsoleOutput(child.stdout,onData),foxStderr=pipeFoxConsoleOutput(child.stderr,onData);signal&&')
    return replace_once(source, 'let exitCode=await waitForChildProcess(child);',
                        'let exitCode=await waitForChildProcess(child);foxStdout();foxStderr();')


def patch_dsh_owner_label(source):
    source = replace_once(source, 'api.getNamedSecurityInfoW(path, 1, 20, ownerSlot',
                          'api.getNamedSecurityInfoW(path, 1, 21, ownerSlot')
    source = replace_once(source, '\t\toldAcl: decodePtr(daclSlot),',
                          '\t\towner: decodePtr(ownerSlot),\n\t\toldAcl: decodePtr(daclSlot),')
    anchor = 'function grantWrite(api, path, sidPtr, lowLabelSidPtr, worldSidPtr) {\n\twithPathLock(api, path, () => {\n\t\tconst { oldAcl, labelAcl, descriptor } = readCurrentSecurity(api, path);'
    return replace_once(source, anchor, '''function foxEnsureOwnerLabelAccess(api, path) {
\t// An owner can edit the DACL but does not implicitly have WRITE_OWNER,
\t// which Windows requires to set the Low label on a Modify-only directory.
\tconst handle = api.createFileW(path, 524288, 7, null, 3, 33554432, null);
\tif (!isInvalidHandle(handle)) {
\t\tif (api.closeHandle(handle) === 0) throwLastError$1(api, "CloseHandle", "owner label access");
\t\treturn;
\t}
\tif (api.getLastError() !== 5) throwLastError$1(api, "CreateFileW", path);
\tconst token = openCurrentProcessToken(api);
\ttry {
\t\tconst size = allocUint32();
\t\tapi.getTokenInformation(token, 1, null, 0, size);
\t\tconst user = Buffer.alloc(decodeUint32(size));
\t\tif (api.getTokenInformation(token, 1, user, user.length, size) === 0) throwLastError$1(api, "GetTokenInformation", "TokenUser");
\t\tconst security = readCurrentSecurity(api, path);
\t\ttry {
\t\t\tif (security.owner === null || !sameSidAt(security.owner, 0, decodePtrAt(user, 0), 0) || security.oldAcl === null) return;
\t\t\tmergeAndApply(api, path, buildExplicitAccess(security.owner, 1, 524288, 0), security.oldAcl,
\t\t\t\t{ kind: "keep" }, null, "owner label access");
\t\t} finally {
\t\t\tif (security.descriptor !== null) api.localFree(security.descriptor);
\t\t}
\t} finally {
\t\tapi.closeHandle(token);
\t}
}
function grantWrite(api, path, sidPtr, lowLabelSidPtr, worldSidPtr) {
\twithPathLock(api, path, () => {
\t\tfoxEnsureOwnerLabelAccess(api, path);
\t\tconst { oldAcl, labelAcl, descriptor } = readCurrentSecurity(api, path);''')


def patch_dsh_proxy(source):
    source = 'import { foxModelProxyTarget } from "./fox-model-proxy.mjs";\n' + source
    return replace_once(source, '\tif (isLoopbackHost(url.hostname)) return void 0;', '\tif (isLoopbackHost(url.hostname) && foxModelProxyTarget(url)) return proxy;\n\tif (isLoopbackHost(url.hostname)) return void 0;')


def patch_dsh_cancel(source):
    source = replace_once(source, '\tactivityDone = Promise.resolve();', '\tactivityDone = Promise.resolve();\n\tfoxUserCancelled = false;')
    source = replace_once(source, '\tsend(message, target, wakeup) {', '''\tsend(message, target, wakeup) {
\t\tif (this.foxUserCancelled) {
\t\t\tif (message.source.kind !== "user") return;
\t\t\tthis.foxUserCancelled = false;
\t\t}''')
    return replace_once(source, '\tcancel(cause, options = {}) {', '''\tcancel(cause, options = {}) {
\t\tif (cause.kind === "user") {
\t\t\t// Set the fence before killing jobs: their settlement can enqueue a followup.
\t\t\tthis.foxUserCancelled = true;
\t\t\tconst jobs = this.ctx.get("jobs");
\t\t\tfor (const job of jobs?.list(this.id) ?? []) {
\t\t\t\tif (job.owner !== this.id || (job.status !== "running" && job.status !== "stopping")) continue;
\t\t\t\ttry {
\t\t\t\t\tjobs.kill(job.id, this.id, "user cancelled the Fox turn");
\t\t\t\t} catch (error) {
\t\t\t\t\tthis.ctx.logger.warn(`Fox: failed to cancel job ${job.id}: ${String(error)}`);
\t\t\t\t}
\t\t\t}
\t\t}''')


def patch_dsh_job_notifications(source):
    # Explicitly killed jobs must not ask the model to retry, even if they
    # settle after the user has already started the next prompt.
    return replace_once(source, 'event.awaited || event.cause === "teardown"', 'event.awaited || event.cause === "kill" || event.cause === "teardown"')


# fox-runtime-dsh-messages: SenseNova's DeepSeek-compatible Messages stream sends
# content_block_start without the "text" / "thinking" field, or with
# "signature": null. dsh 0.2.1-rc.2 requires strings there and fails the whole
# turn with "DeepSeek Messages expected a string field". Read a missing or null
# field as the empty string (the deltas that follow carry the content); a
# missing signature stays absent, as upstream.
DSH_MESSAGES_START_FIELDS = [
    ('\t\t\t\ttext: string(native.text)\n', '\t\t\t\ttext: string(native.text ?? "")\n'),
    ('\t\t\t\ttext: string(native.thinking)\n', '\t\t\t\ttext: string(native.thinking ?? "")\n'),
    ('...native.signature === void 0 ? {} : { signature: string(native.signature) }', '...native.signature === void 0 ? {} : { signature: string(native.signature ?? "") }'),
]


def patch_dsh_messages(source):
    for old, new in DSH_MESSAGES_START_FIELDS:
        source = replace_once(source, old, new)
    return source


def patch_pi_dispatcher(source):
    return replace_once(source, 'new undici.EnvHttpProxyAgent({', 'new undici.EnvHttpProxyAgent({\n        ...(process.env.FOX_MODEL_PROXY_REQUIRED === "1" ? { noProxy: "" } : {}),')


# Source hashes are npm's published JavaScript, LF-normalized. Versions are
# checked separately. Hashes are not inferred from whatever happens to be installed.
PATCHES = [
    {"id": "pi-acp-errors", "package": "backends/pi/node_modules/pi-acp", "version": "0.0.34", "file": "dist/index.js", "sha256": "324aeb8bba1228937e16b1326fb3e014e2a625b2dea549dbcd49b006cd5df6a2", "patch": patch_pi_acp_all, "helper": ("pi-turn-outcome.mjs", "dist/fox-turn-outcome.mjs")},
    {"id": "dsh-model-proxy", "package": "backends/dsh/node_modules/@deepseek-ai/dsh-http-proxy", "version": "0.2.1-rc.2", "file": "lib/index.js", "sha256": "5310861bc89788ea24be82a3b323e34cbd42b9e6214e2a179b2e6f92a8758cd5", "patch": patch_dsh_proxy, "helper": ("dsh-model-proxy.mjs", "lib/fox-model-proxy.mjs")},
    {"id": "dsh-session-cancel", "package": "backends/dsh/node_modules/@deepseek-ai/dsh-agent-loop", "version": "0.2.1-rc.2", "file": "lib/index.js", "sha256": "cf57e3ecd487ac5c2553455aaa5cef2c6fb41e6bd61be5aa455675d1d7193033", "patch": patch_dsh_cancel},
    {"id": "dsh-killed-job-notifications", "package": "backends/dsh/node_modules/@deepseek-ai/dsh-tool-jobs", "version": "0.2.1-rc.2", "file": "lib/index.js", "sha256": "de5707b43480ba37c8e141e6f67674978f3a46d87d58390c18eb1659bb17ec2f", "patch": patch_dsh_job_notifications},


    {"id": "dsh-messages-stream", "package": "backends/dsh/node_modules/@deepseek-ai/dsh-llm-deepseek", "version": "0.2.1-rc.2", "file": "lib/index.js", "sha256": "eba35025ed1eedda89c51f2da46a3a968f7be1d59719f2ceeadfc4518dff37d6", "patch": patch_dsh_messages},
    {"id": "pi-model-proxy", "package": "backends/pi/node_modules/@earendil-works/pi-coding-agent", "version": "1.4.0", "file": "dist/core/http-dispatcher.js", "sha256": "f9aa2c81b0a5958ffba6368506f24c9b202904c234ada19494cdc9c4a1d0e97b", "patch": patch_pi_dispatcher},
    {"id": "pi-bundled-model-proxy", "package": "backends/pi/node_modules/@earendil-works/pi-coding-agent", "version": "1.4.0", "file": "dist/bundle/chunks/chunk-MM3LKT73.js", "sha256": "aa4c8b091657bda611f6fd58343561fa44cda1070ae4cd5dbd09ffe1d9a34aab", "patch": patch_pi_bundle_all, "helper": ("pi-console-output.mjs", "dist/bundle/chunks/fox-console-output.mjs")},
    {"id": "pi-shell-console", "package": "backends/pi/node_modules/@earendil-works/pi-coding-agent", "version": "1.4.0", "file": "dist/core/tools/bash.js", "sha256": "f32e87a42f66276b2eafaaf0848babf52523d37ab0fd448439586376747f03c4", "patch": patch_pi_shell_console, "helper": ("pi-console-output.mjs", "dist/core/tools/fox-console-output.mjs")},
    {"id": "dsh-owner-label-access", "package": "backends/dsh/node_modules/@deepseek-ai/dsh-sandbox-windows-acl", "version": "0.2.1-rc.2", "file": "lib/types-Cl_DXjhk.js", "sha256": "0bb3a34508785bdc34626542f9358d398fd9f60fdaf4b48201dfdff2122f25fc", "patch": patch_dsh_owner_label},
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
