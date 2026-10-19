/**
 * TEST ONLY (tests/sandbox_srt.rs). Registers the provider `foxcmd` / model
 * `scripted` for driving real pi 0.84.2 + real pi-acp 0.0.34 inside srt
 * without credentials.
 *
 * Client only: the scripted "model" (an OpenAI Chat Completions compatible
 * server) runs in the test process, OUTSIDE the sandbox, and its base URL
 * arrives here as FOX_SCRIPTED_MODEL_URL (e.g. http://127.0.0.1:53111/v1).
 * The test lists exactly that 127.0.0.1:<port> in the srt network allowlist,
 * so pi reaches it through the srt proxy like any model API. (Up to round 5
 * this file started the server itself, inside pi, on a random port that was
 * not in the allowlist; on Windows srt drops NO_PROXY, so the request went to
 * the proxy and came back "403 Connection blocked by network allowlist".)
 *
 * The script the server follows (see ScriptedModel in tests/sandbox_srt.rs):
 *  - RUN:<base64url command>  -> one `bash` tool call running that command
 *  - READ:<base64url path>    -> one `read` tool call of that path
 *  - last message is a tool result -> text "tool-result: <first 400 chars>"
 *  - anything else -> text "scripted hello"
 *
 * Never load this outside tests.
 */
export default function scriptedCmdProvider(pi: any) {
	const baseUrl = process.env.FOX_SCRIPTED_MODEL_URL;
	if (!baseUrl || !/^http:\/\/127\.0\.0\.1:\d+\/v1$/.test(baseUrl)) {
		throw new Error(`FOX_SCRIPTED_MODEL_URL must be http://127.0.0.1:<port>/v1 (got ${JSON.stringify(baseUrl)})`);
	}
	pi.registerProvider("foxcmd", {
		name: "Fox scripted command model (test only)",
		baseUrl,
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
