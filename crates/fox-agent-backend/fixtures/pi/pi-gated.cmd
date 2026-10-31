@echo off
rem pi command for pi-acp on Windows (set PI_ACP_PI_COMMAND to this file; the
rem Unix twin is pi-gated.sh). Runs the pi binary Fox pins (FOX_PI_BIN, e.g.
rem <prefix>\node_modules\.bin\pi.cmd of @earendil-works/pi-coding-agent
rem 0.84.2) with extension discovery off and only the Fox approval gate
rem loaded. pi-acp spawns *.cmd through cross-spawn and appends
rem `--mode rpc --no-themes [--session <file>]`; extra flags for tests come
rem from FOX_PI_EXTRA_ARGS (word-split, no paths: Fox passes variables to
rem the sandbox as `set "K=V"`, which cannot carry quotes) and one optional
rem extra extension path in FOX_PI_TEST_EXTENSION (quoted here; test use only).
rem The gate must sit next to this file (%~dp0 ends with a backslash).
rem No labels / goto: the repository stores LF line endings (.gitattributes),
rem and cmd.exe label lookup is unreliable in LF-only batch files.
setlocal
if not defined FOX_PI_BIN set "FOX_PI_BIN=pi"
set "FOX_PI_TEST_EXT_ARGS="
if defined FOX_PI_TEST_EXTENSION set "FOX_PI_TEST_EXT_ARGS=-e "%FOX_PI_TEST_EXTENSION%""
rem Plan C3: when Fox sets FOX_PI_MCP_EXTENSION, the Fox tool extension (MCP
rem bridge: present_files, knowledge, ask_user) is loaded after the gate.
set "FOX_PI_MCP_EXT_ARGS="
if defined FOX_PI_MCP_EXTENSION set "FOX_PI_MCP_EXT_ARGS=-e "%FOX_PI_MCP_EXTENSION%""
"%FOX_PI_BIN%" --no-extensions -e "%~dp0fox-approval-gate.ts" %FOX_PI_MCP_EXT_ARGS% %FOX_PI_TEST_EXT_ARGS% %FOX_PI_EXTRA_ARGS% %*
