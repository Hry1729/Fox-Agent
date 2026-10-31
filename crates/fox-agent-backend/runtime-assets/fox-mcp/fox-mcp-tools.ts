/**
 * Fox tool MCP bridge for pi (plan C3).
 *
 * pi-acp 0.0.34 stores ACP `mcpServers` but never wires them to pi, so pi
 * reaches the Fox Host tools through this extension instead. The gated pi
 * (pi-gated.sh / pi-gated.cmd) loads it with `-e "$FOX_PI_MCP_EXTENSION"`
 * when Fox sets that variable. The Fox Host writes this file into its own
 * state directory (fox_mcp::write_assets) and points FOX_PI_MCP_EXTENSION at
 * that copy; the copy inside the runtime package is not what runs.
 *
 * At startup (async factory: pi waits for it before it answers pi-acp's
 * first RPC, so pi-acp's session/new waits too) the extension connects to the
 * Host bridge (FOX_MCP_ENDPOINT, first line {"foxMcpToken": FOX_MCP_TOKEN}),
 * runs MCP `initialize` + `tools/list` and registers every listed tool with
 * pi. A tool call becomes MCP `tools/call`; the Host runs it through the same
 * entry as a PiNative call and returns the (64 KiB capped) result. The Fox
 * approval gate still holds each of these calls first; the Host answers those
 * gate requests itself and makes the real decision at the bridge.
 *
 * Fail loudly and fast, never hang (fox-fix-pi):
 *  - connect + initialize + tools/list must finish within the init deadline
 *    (INIT_TIMEOUT_MS, FOX_MCP_INIT_TIMEOUT_MS overrides it for tests); a
 *    bridge that accepts the connection but never answers is an error, not a
 *    pi that waits forever (pi-acp's session/new has no timeout of its own);
 *  - no bridge, a refused token or a failed handshake throws from the factory
 *    with `fox_tools_unavailable`, so pi does not start without the Fox tools
 *    Fox promised (the Fox Host then reports
 *    agent_backend_fox_tools_unavailable, see fox_mcp::McpProbe);
 *  - the socket holds pi's event loop only while a request is in flight, so
 *    `pi -p` (print mode) still exits after its answer;
 *  - a tool error is thrown from `execute` (pi reports it to the model as an
 *    error); an aborted turn rejects the call at once.
 */
import net from "node:net";

const endpoint = process.env.FOX_MCP_ENDPOINT || "";
const token = process.env.FOX_MCP_TOKEN || "";
const INIT_TIMEOUT_MS = (() => {
	const value = Number(process.env.FOX_MCP_INIT_TIMEOUT_MS);
	return Number.isFinite(value) && value > 0 ? value : 20_000;
})();
const CODE = "fox_tools_unavailable";

type Pending = { resolve: (value: any) => void; reject: (error: Error) => void };
type Bridge = {
	request: (method: string, params?: any, signal?: AbortSignal) => Promise<any>;
	notify: (method: string, params?: any) => void;
	close: () => void;
};

function unavailable(message: string): Error {
	return new Error(`${CODE}: Fox tool bridge ${message}`);
}

function connectBridge(): Promise<Bridge> {
	const match = /^127\.0\.0\.1:(\d{1,5})$/.exec(endpoint);
	if (!match || !/^[0-9a-f]{64}$/.test(token)) {
		return Promise.reject(unavailable("FOX_MCP_ENDPOINT / FOX_MCP_TOKEN missing or malformed (set by Fox)"));
	}
	return new Promise((resolveConnection, rejectConnection) => {
		const socket = net.connect({ host: "127.0.0.1", port: Number(match[1]) });
		const pending = new Map<number, Pending>();
		let nextId = 1;
		let buffer = "";
		let closedError: Error | null = null;
		let connected = false;
		// Hold pi's event loop only while a request waits for its answer.
		const holdLoop = () => (pending.size > 0 ? socket.ref() : socket.unref());
		const failAll = (error: Error) => {
			closedError ??= error;
			for (const entry of pending.values()) entry.reject(closedError);
			pending.clear();
			holdLoop();
		};
		socket.setEncoding("utf8");
		socket.setNoDelay(true);
		socket.on("data", (chunk: string) => {
			buffer += chunk;
			let newline = buffer.indexOf("\n");
			while (newline >= 0) {
				const line = buffer.slice(0, newline).trim();
				buffer = buffer.slice(newline + 1);
				newline = buffer.indexOf("\n");
				if (!line) continue;
				let message: any;
				try {
					message = JSON.parse(line);
				} catch {
					continue;
				}
				if (message?.id === null && message?.error) {
					failAll(unavailable(`refused: ${message.error.message}`));
					socket.destroy();
					continue;
				}
				const entry = pending.get(message?.id);
				if (!entry) continue;
				pending.delete(message.id);
				holdLoop();
				if (message.error) entry.reject(new Error(`Fox tool bridge: ${message.error.message} (${message.error.code})`));
				else entry.resolve(message.result);
			}
		});
		socket.on("error", (error: Error) => {
			failAll(unavailable(`unreachable at ${endpoint}: ${error.message}`));
			if (!connected) rejectConnection(closedError);
		});
		socket.on("close", () => failAll(closedError ?? new Error("Fox tool bridge closed (the Fox Host stopped this pi process)")));
		socket.on("connect", () => {
			connected = true;
			socket.write(JSON.stringify({ foxMcpToken: token }) + "\n");
			const send = (message: any) => socket.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n");
			resolveConnection({
				request: (method, params, signal) =>
					new Promise((resolve, reject) => {
						if (closedError) return reject(closedError);
						if (signal?.aborted) return reject(new Error("Turn aborted before the Fox tool ran"));
						const id = nextId++;
						const onAbort = () => {
							if (!pending.delete(id)) return;
							holdLoop();
							send({ method: "notifications/cancelled", params: { requestId: id, reason: "turn aborted" } });
							reject(new Error("Turn aborted while the Fox tool was running"));
						};
						signal?.addEventListener("abort", onAbort, { once: true });
						pending.set(id, {
							resolve: (value) => {
								signal?.removeEventListener("abort", onAbort);
								resolve(value);
							},
							reject: (error) => {
								signal?.removeEventListener("abort", onAbort);
								reject(error);
							},
						});
						holdLoop();
						send({ id, method, params });
					}),
				notify: (method, params) => send({ method, params }),
				close: () => socket.destroy(),
			});
		});
	});
}

/** connect + initialize + tools/list, bounded by INIT_TIMEOUT_MS. */
async function handshake(): Promise<{ bridge: Bridge; tools: any[] }> {
	let bridge: Bridge | null = null;
	let timer: ReturnType<typeof setTimeout> | undefined;
	const deadline = new Promise<never>((_, reject) => {
		timer = setTimeout(
			() => reject(unavailable(`did not finish connect / initialize / tools/list within ${INIT_TIMEOUT_MS} ms (endpoint ${endpoint})`)),
			INIT_TIMEOUT_MS,
		);
	});
	const steps = (async () => {
		bridge = await connectBridge();
		await bridge.request("initialize", {
			protocolVersion: "2025-06-18",
			capabilities: {},
			clientInfo: { name: "fox-pi-extension", version: "2" },
		});
		bridge.notify("notifications/initialized");
		const listed = await bridge.request("tools/list", {});
		const tools: any[] = Array.isArray(listed?.tools) ? listed.tools : [];
		if (tools.length === 0) throw unavailable("listed no tools");
		return { bridge, tools };
	})();
	try {
		return await Promise.race([steps, deadline]);
	} catch (error: any) {
		(bridge as Bridge | null)?.close();
		steps.catch(() => {});
		const message = String(error?.message ?? error);
		throw message.startsWith(CODE) ? error : unavailable(`handshake failed: ${message}`);
	} finally {
		clearTimeout(timer);
	}
}

export default async function foxMcpTools(pi: any) {
	const { bridge, tools } = await handshake();
	for (const tool of tools) {
		pi.registerTool({
			name: String(tool.name),
			label: String(tool.title ?? tool.name),
			description: String(tool.description ?? ""),
			// Plain JSON Schema: pi validates non-TypeBox schemas as JSON Schema.
			parameters: tool.inputSchema ?? { type: "object", properties: {} },
			async execute(_toolCallId: string, params: any, signal?: AbortSignal) {
				const result = await bridge.request("tools/call", { name: tool.name, arguments: params ?? {} }, signal);
				const content = Array.isArray(result?.content) ? result.content : [{ type: "text", text: JSON.stringify(result ?? null) }];
				if (result?.isError) {
					throw new Error(content.map((block: any) => (block?.type === "text" ? block.text : "")).join("\n") || "Fox tool failed");
				}
				return { content, details: { foxMcp: true } };
			},
		});
	}
}
