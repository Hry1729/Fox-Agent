/**
 * TEST ONLY. A deterministic "model" for exercising real pi 0.84.2 + real
 * pi-acp 0.0.34 without model credentials. It starts an OpenAI Chat
 * Completions compatible HTTP server inside the pi process (127.0.0.1, random
 * port) and registers it as provider `foxscripted`, model `scripted`. pi talks
 * to it through its normal `openai-completions` code path; nothing in pi or
 * pi-acp is stubbed. Never load this outside tests.
 *
 * Script (decided from the request messages):
 *  - last message is a tool result  -> text "tool-result: <first 200 chars>"
 *  - last user text contains GATE:<token> -> one bash tool call
 *      `echo <token> > fox-proof-<token>.txt`
 *  - GATEWRITE:<token> -> one `write` tool call creating fox-write-<token>.txt
 *  - GATEREAD:<token> -> one `read` tool call of fox-write-<token>.txt
 *  - GATESLEEP:<token> -> one bash call `sleep 299; echo fox-sleep-<token>`
 *      (long-running, for cancel / process-tree measurements)
 *  - anything else -> text "scripted hello"
 */
import * as http from "node:http";

function sse(res: any, chunk: unknown) {
	res.write(`data: ${JSON.stringify(chunk)}\n\n`);
}

function textOf(content: unknown): string {
	if (typeof content === "string") return content;
	if (Array.isArray(content)) {
		return content.map((part: any) => (typeof part?.text === "string" ? part.text : "")).join("");
	}
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
		text(`tool-result: ${textOf(last.content).slice(0, 200)}`);
		return;
	}
	const lastUser = [...messages].reverse().find((m) => m?.role === "user");
	const match = /(GATE|GATEWRITE|GATEREAD|GATESLEEP):([A-Za-z0-9_-]+)/.exec(textOf(lastUser?.content));
	if (match) {
		const kind = match[1];
		const token = match[2];
		let name = "bash";
		let args = JSON.stringify({ command: `echo ${token} > fox-proof-${token}.txt` });
		if (kind === "GATEWRITE") {
			name = "write";
			args = JSON.stringify({ path: `fox-write-${token}.txt`, content: token });
		} else if (kind === "GATEREAD") {
			name = "read";
			args = JSON.stringify({ path: `fox-write-${token}.txt` });
		} else if (kind === "GATESLEEP") {
			args = JSON.stringify({ command: `sleep 299; echo fox-sleep-${token}` });
		}
		sse(res, {
			...base,
			choices: [
				{
					index: 0,
					delta: {
						role: "assistant",
						tool_calls: [{ index: 0, id: `call_${kind === "GATEREAD" ? "read_" : ""}${token}`, type: "function", function: { name, arguments: args } }],
					},
					finish_reason: null,
				},
			],
		});
		finish("tool_calls");
		return;
	}
	text("scripted hello");
}

export default async function scriptedProvider(pi: any) {
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
	pi.registerProvider("foxscripted", {
		name: "Fox scripted (test only)",
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
