/**
 * Fox tool MCP bridge for pi (plan C3).
 *
 * pi-acp 0.0.34 stores ACP `mcpServers` but never wires them to pi, so pi
 * reaches the Fox Host tools through this extension instead. The gated pi
 * (pi-gated.sh / pi-gated.cmd) loads it with `-e "$FOX_PI_MCP_EXTENSION"`
 * when Fox sets that variable.
 *
 * At startup (async factory: pi waits for it) the extension connects to the
 * Host bridge (FOX_MCP_ENDPOINT, first line {"foxMcpToken": FOX_MCP_TOKEN}),
 * runs MCP `initialize` + `tools/list` and registers every listed tool with
 * pi. A tool call becomes MCP `tools/call`; the Host runs it through the same
 * entry as a PiNative call and returns the (64 KiB capped) result. The Fox
 * approval gate still holds each of these calls first; the Host answers those
 * gate requests itself and makes the real decision at the bridge.
 *
 * Fail loudly: no bridge, a refused token, or a failed handshake throws from
 * the factory, so pi does not start without the Fox tools Fox promised; a
 * tool error is thrown from `execute` (pi reports it to the model as an error).
 */
import net from "node:net";

const endpoint = process.env.FOX_MCP_ENDPOINT || "";
const token = process.env.FOX_MCP_TOKEN || "";

type Pending = { resolve: (value: any) => void; reject: (error: Error) => void };

function connectBridge(): Promise<{ request: (method: string, params?: any) => Promise<any>; notify: (method: string, params?: any) => void }> {
	const match = /^127\.0\.0\.1:(\d{1,5})$/.exec(endpoint);
	if (!match || !/^[0-9a-f]{64}$/.test(token)) {
		return Promise.reject(new Error("Fox tool bridge: FOX_MCP_ENDPOINT / FOX_MCP_TOKEN missing or malformed (set by Fox)"));
	}
	return new Promise((resolveConnection, rejectConnection) => {
		const socket = net.connect({ host: "127.0.0.1", port: Number(match[1]) });
		const pending = new Map<number, Pending>();
		let nextId = 1;
		let buffer = "";
		let closedError: Error | null = null;
		const failAll = (error: Error) => {
			closedError = error;
			for (const entry of pending.values()) entry.reject(error);
			pending.clear();
		};
		socket.setEncoding("utf8");
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
					failAll(new Error(`Fox tool bridge refused: ${message.error.message}`));
					socket.destroy();
					continue;
				}
				const entry = pending.get(message?.id);
				if (!entry) continue;
				pending.delete(message.id);
				if (message.error) entry.reject(new Error(`Fox tool bridge: ${message.error.message} (${message.error.code})`));
				else entry.resolve(message.result);
			}
		});
		socket.on("error", (error: Error) => {
			failAll(new Error(`Fox tool bridge unreachable at ${endpoint}: ${error.message}`));
			rejectConnection(closedError);
		});
		socket.on("close", () => failAll(closedError ?? new Error("Fox tool bridge closed (the Fox Host stopped this pi process)")));
		socket.on("connect", () => {
			socket.write(JSON.stringify({ foxMcpToken: token }) + "\n");
			const send = (message: any) => socket.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\n");
			resolveConnection({
				request: (method, params) =>
					new Promise((resolve, reject) => {
						if (closedError) return reject(closedError);
						const id = nextId++;
						pending.set(id, { resolve, reject });
						send({ id, method, params });
					}),
				notify: (method, params) => send({ method, params }),
			});
		});
	});
}

export default async function foxMcpTools(pi: any) {
	const bridge = await connectBridge();
	await bridge.request("initialize", {
		protocolVersion: "2025-06-18",
		capabilities: {},
		clientInfo: { name: "fox-pi-extension", version: "1" },
	});
	bridge.notify("notifications/initialized");
	const listed = await bridge.request("tools/list", {});
	const tools: any[] = Array.isArray(listed?.tools) ? listed.tools : [];
	if (tools.length === 0) throw new Error("Fox tool bridge listed no tools");
	for (const tool of tools) {
		pi.registerTool({
			name: String(tool.name),
			label: String(tool.title ?? tool.name),
			description: String(tool.description ?? ""),
			// Plain JSON Schema: pi validates non-TypeBox schemas as JSON Schema.
			parameters: tool.inputSchema ?? { type: "object", properties: {} },
			async execute(_toolCallId: string, params: any, signal?: AbortSignal) {
				if (signal?.aborted) throw new Error("Turn aborted before the Fox tool ran");
				const result = await bridge.request("tools/call", { name: tool.name, arguments: params ?? {} });
				const content = Array.isArray(result?.content) ? result.content : [{ type: "text", text: JSON.stringify(result ?? null) }];
				if (result?.isError) {
					throw new Error(content.map((block: any) => (block?.type === "text" ? block.text : "")).join("\n") || "Fox tool failed");
				}
				return { content, details: { foxMcp: true } };
			},
		});
	}
}
