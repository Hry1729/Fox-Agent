#!/usr/bin/env node
// TEST launcher: real `npx -y pi-acp@<version>` with the Fox approval gate wired
// in through PI_ACP_PI_COMMAND. In production the Host would inject these
// variables itself (they are not in launch.rs ENV_ALLOWLIST today; see
// docs/pi.md "Known gaps").
//
//   node launch-pi-acp.mjs --home <dir> --pi-bin <path to pi> [--extra "<pi args>"] [--version 0.0.34] [--no-gate]
//
// --home isolates HOME so pi's ~/.pi/agent (auth, sessions, settings) and
// pi-acp's ~/.pi/pi-acp/session-map.json never touch the real user profile.
import { spawn } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const opt = (name, fallback) => {
	const index = args.indexOf(name);
	return index >= 0 ? args[index + 1] : fallback;
};
const home = opt("--home");
const piBin = opt("--pi-bin");
const extra = opt("--extra", "");
const version = opt("--version", "0.0.34");
const gate = !args.includes("--no-gate");

const env = { ...process.env };
if (home) env.HOME = home;
if (piBin) {
	env.FOX_PI_BIN = piBin;
	// pi-acp also runs bare `pi --version` for its startup banner.
	env.PATH = `${dirname(piBin)}${process.platform === "win32" ? ";" : ":"}${env.PATH ?? ""}`;
}
if (gate) {
	let gated = join(here, "pi-gated.sh");
	// The repository stores the script without its executable bit (files
	// committed through the contents API are 100644) and pi-acp spawns it
	// directly (EACCES otherwise). Run a private executable copy, next to a
	// copy of the gate it loads, instead of changing the checkout.
	if (process.platform !== "win32") {
		const dir = home ? join(home, ".fox-pi-launcher") : mkdtempSync(join(tmpdir(), "fox-pi-launcher-"));
		mkdirSync(dir, { recursive: true });
		for (const file of ["pi-gated.sh", "fox-approval-gate.ts"]) copyFileSync(join(here, file), join(dir, file));
		gated = join(dir, "pi-gated.sh");
		chmodSync(gated, 0o755);
	}
	env.PI_ACP_PI_COMMAND = gated;
}
if (extra) env.FOX_PI_EXTRA_ARGS = extra;

const npx = process.platform === "win32" ? "npx.cmd" : "npx";
const child = spawn(npx, ["-y", `pi-acp@${version}`], { stdio: "inherit", env });
for (const signal of ["SIGTERM", "SIGINT"]) process.on(signal, () => child.kill(signal));
child.on("exit", (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
