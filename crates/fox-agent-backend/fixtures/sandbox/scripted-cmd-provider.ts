/**
 * TEST ONLY (tests/sandbox_srt.rs). A deterministic in-process "model" for
 * driving real pi 0.84.2 + real pi-acp 0.0.34 inside srt without credentials.
 * Same mechanism as fixtures/pi/scripted-provider.ts (OpenAI Chat Completions
 * compatible server on 127.0.0.1 inside the pi process, provider `foxcmd`,
 * model `scripted`), but the tool call is taken from the prompt so a test can
 * probe the sandbox boundary:
 *
 *  - RUN:<base64url command>  -> one `bash` tool call running that command
 *  - READ:<base64url path>    -> one `read` tool call of that path
 *  - last message is a tool result -> text "tool-result: <first 400 chars>"
 *  - anything else -> text "scripted hello"
 *
 * Never load this outside tests.
 */
import * as http from "node:http";

function sse(res: any, chunk: unknown) {
	res.write(`data: ${JSON.stringify(chunk)}\n\n`);
}

function textOf(content: unknown): string {
	if (typeof content === "string") return content;
	if (Array.isArray(content)) return content.map((part: any) => (typeof part?.text === "string" ? part.text : "")).join("");
	return "";
}

function respond(body: any, res: any) {
	const messages: any[] = Array.isArray(body?.messages) ? body.messages : [];
	const last = messages[messages.length - 1];
	const base = { id: "scripted-1", object: "chat.completion.chunk", created: 0, model: "scripted" };
	res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
	const finish = (reason: string) => {
		sse(res, { ...base, choices: [{ index: 0, delta: {}, finish_reason: reason }] });
		sse(res, { ...base, choices: [], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } });
		res.write("data: [DONE]\n\n");
		res.end();
	};
	const text = (value: string) => {
		sse(res, { ...base, choices: [{ index: 0, delta: { role: "assistant", content: value }, finish_reason: null }] });
		finish("stop");
	};
	if (last?.role === "tool") {
		text(`tool-result: ${textOf(last.content).slice(0, 400)}`);
		return;
	}
	const lastUser = [...messages].reverse().find((m) => m?.role === "user");
	const match = /(RUN|READ):([A-Za-z0-9_-]+)(?:\s+ID:([A-Za-z0-9_-]+))?/.exec(textOf(lastUser?.content));
	if (!match) {
		text("scripted hello");
		return;
	}
	const decoded = Buffer.from(match[2], "base64url").toString("utf8");
	const id = match[3] ?? "x";
	const call =
		match[1] === "RUN"
			? { name: "bash", args: JSON.stringify({ command: decoded }) }
			: { name: "read", args: JSON.stringify({ path: decoded }) };
	sse(res, {
		...base,
		choices: [
			{
				index: 0,
				delta: {
					role: "assistant",
					tool_calls: [{ index: 0, id: `call_${id}`, type: "function", function: { name: call.name, arguments: call.args } }],
				},
				finish_reason: null,
			},
		],
	});
	finish("tool_calls");
}

export default async function scriptedCmdProvider(pi: any) {
	const server = http.createServer((req, res) => {
		let raw = "";
		req.on("data", (chunk) => (raw += chunk));
		req.on("end", () => {
			let body: any = {};
			try {
				body = JSON.parse(raw || "{}");
			} catch {}
			if (req.url?.endsWith("/models")) {
				res.writeHead(200, { "content-type": "application/json" });
				res.end(JSON.stringify({ data: [{ id: "scripted" }] }));
				return;
			}
			respond(body, res);
		});
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
	server.unref();
	const port = (server.address() as any).port;
	pi.registerProvider("foxcmd", {
		name: "Fox scripted command model (test only)",
		baseUrl: `http://127.0.0.1:${port}/v1`,
		apiKey: "scripted-not-a-secret",
		api: "openai-completions",
		models: [
			{
				id: "scripted",
				name: "Scripted",
				reasoning: false,
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 32000,
				maxTokens: 1024,
				compat: { supportsDeveloperRole: false, maxTokensField: "max_tokens" },
			},
		],
	});
}
