// Excerpt of @deepseek-ai/dsh-llm-deepseek 0.2.1-rc.2 lib/index.js (MIT License,
// Copyright DeepSeek; https://github.com/deepseek-ai/deepseek-harness), verbatim:
// lines 1479-1482 (object), 1488-1497 (replayState), 1734-2021 (transport, sse, translate).
// Test fixture for runtime_patches.patch_dsh_messages; imports are supplied by the test harness.
function object(value, code = "MALFORMED_RESPONSE") {
	if (typeof value !== "object" || value === null || Array.isArray(value)) throw new LlmError("DeepSeek Messages expected a JSON object", code);
	return value;
}
function replayState(model, blocks) {
	return {
		response: {
			kind: "deepseek-messages",
			version: 1,
			model
		},
		blocks
	};
}
//#region lib/types/transport.js
/** Normalize HTTP and in-band Messages errors into provider-neutral failures. */
/** Read only provider error fields used by bounded Files recovery.
* @param raw - decoded HTTP error response.
* @returns code, type, and message text, without unrelated response fields.
*/
function providerErrorDetail(raw) {
	const error = typeof raw === "object" && raw !== null && "error" in raw ? raw.error : void 0;
	if (typeof error !== "object" || error === null) return "";
	const fields = error;
	return [
		fields.code,
		fields.type,
		fields.message
	].filter((value) => typeof value === "string").join(" ");
}
/** Classify a provider error without trusting arbitrary response fields.
* @param raw - decoded response or in-band error event.
* @param status - HTTP status when the error preceded streaming.
* @param headers - response headers for retry delay and request identity.
* @returns a stable error consumed by LlmRuntime and llm-retry.
*/
function providerError(raw, status, headers) {
	const envelope = typeof raw === "object" && raw !== null ? raw : {};
	const error = typeof envelope.error === "object" && envelope.error !== null ? envelope.error : {};
	const message = typeof error.message === "string" ? error.message : `DeepSeek Messages request failed (${status ?? "stream error"})`;
	const type = typeof error.type === "string" ? error.type : "";
	const detail = `${type} ${typeof error.code === "string" ? error.code : ""} ${message}`;
	let code;
	if (status === 401 || status === 403 || ["authentication_error", "permission_error"].includes(type)) code = "AUTH";
	else if (isQuotaExceededError(detail) || status === 402) code = "QUOTA";
	else if (status === 429 || type === "rate_limit_error") code = "RATE_LIMIT";
	else if (isContextWindowExceededError(detail)) code = "CONTEXT_WINDOW_EXCEEDED";
	else if (status === 400 || status === 413 || type === "invalid_request_error") code = "INVALID_REQUEST";
	else if (status !== void 0 && status >= 500 || ["api_error", "overloaded_error"].includes(type)) code = "SERVER";
	else code = status === void 0 ? "SERVER" : `HTTP_${status}`;
	const retry = headers?.get("retry-after");
	const delay = retry == null ? NaN : /^\d+(?:\.\d+)?$/u.test(retry) ? Number(retry) * 1e3 : Date.parse(retry) - Date.now();
	const id = headers?.get("request-id") ?? headers?.get("x-request-id") ?? headers?.get("x-deepseek-request-id");
	return new LlmError(message, code, {
		...status === void 0 ? {} : { status },
		...id ? { requestId: ProviderRequestId(id) } : {},
		...Number.isFinite(delay) && delay > 0 ? { providerRetryAfterMs: delay } : {}
	});
}
//#endregion
//#region lib/types/sse.js
/** SSE framing delegated to eventsource-parser; JSON errors remain provider failures. */
/** Decode complete SSE frames without treating an unterminated tail as an event.
* @param body - provider response bytes.
* @param activity - pulse the idle watchdog for events and heartbeat comments.
* @returns JSON events, including message_stop; the translator owns completion.
*/
async function* parseSse(body, activity) {
	const events = body.pipeThrough(new TextDecoderStream()).pipeThrough(new EventSourceParserStream({ onComment: activity }));
	for await (const frame of events) {
		activity();
		let raw;
		try {
			raw = JSON.parse(frame.data);
		} catch (_invalidSseJson) {
			throw new LlmError("DeepSeek Messages SSE contains invalid JSON", "MALFORMED_RESPONSE");
		}
		const event = object(raw);
		if (typeof event.type !== "string" || frame.event !== void 0 && frame.event !== event.type) throw new LlmError("DeepSeek Messages SSE event type mismatch", "MALFORMED_RESPONSE");
		if (event.type === "error") throw providerError(event, void 0);
		yield event;
	}
}
//#endregion
//#region lib/types/translate.js
/** Translate Messages events while preserving block order and cumulative usage. */
/** Decode a required string from provider JSON.
* @param value - provider field.
* @returns the validated string.
*/
function string(value) {
	if (typeof value !== "string") throw new LlmError("DeepSeek Messages expected a string field", "MALFORMED_RESPONSE");
	return value;
}
function malformed(detail) {
	throw new LlmError(`DeepSeek Messages stream: ${detail}`, "MALFORMED_RESPONSE");
}
function indexOf(event) {
	if (!Number.isSafeInteger(event.index) || event.index < 0) return malformed("invalid block index");
	return event.index;
}
function updateUsage(usage, raw) {
	const fields = object(raw);
	for (const [wire, local] of Object.entries({
		input_tokens: "inputTokens",
		output_tokens: "outputTokens",
		cache_read_input_tokens: "cacheReadTokens",
		cache_creation_input_tokens: "cacheWriteTokens"
	})) {
		const value = fields[wire];
		if (value === void 0) continue;
		if (!Number.isSafeInteger(value) || value < 0) return malformed(`invalid ${wire}`);
		usage[local] = value;
	}
}
function startBlock(event, index) {
	const native = object(event.content_block);
	let content;
	let replay;
	switch (native.type) {
		case "text":
			content = {
				type: "text",
				text: string(native.text)
			};
			replay = { type: "text" };
			break;
		case "thinking":
			content = {
				type: "reasoning",
				text: string(native.thinking)
			};
			replay = {
				type: "reasoning",
				...native.signature === void 0 ? {} : { signature: string(native.signature) }
			};
			break;
		case "tool_use":
			content = {
				type: "tool-call",
				id: ToolCallId(string(native.id)),
				name: string(native.name),
				arguments: JSON.stringify(object(native.input))
			};
			if (!content.id || !content.name) return malformed("empty tool identity");
			replay = { type: "tool-call" };
			break;
		default: throw new LlmError(`DeepSeek Messages does not support response block ${String(native.type)}`, "UNSUPPORTED_CONTENT");
	}
	return {
		index,
		content,
		replay,
		closed: false,
		json: ""
	};
}
function deltaChunk(block, raw) {
	const delta = object(raw);
	const content = block.content;
	if (delta.type === "text_delta" && content.type === "text") {
		const text = string(delta.text);
		content.text += text;
		return {
			type: "text-delta",
			index: block.index,
			text
		};
	}
	if (delta.type === "thinking_delta" && content.type === "reasoning") {
		const text = string(delta.thinking);
		content.text += text;
		return {
			type: "reasoning-delta",
			index: block.index,
			text
		};
	}
	if (delta.type === "signature_delta" && content.type === "reasoning") {
		block.replay.signature = (block.replay.signature ?? "") + string(delta.signature);
		return;
	}
	if (delta.type === "input_json_delta" && content.type === "tool-call") {
		const argumentsDelta = string(delta.partial_json);
		block.json += argumentsDelta;
		return {
			type: "tool-call-delta",
			index: block.index,
			id: content.id,
			argumentsDelta
		};
	}
	return malformed(`unsupported delta ${String(delta.type)} for ${content.type}`);
}
function stopReason(raw) {
	switch (raw) {
		case "end_turn":
		case "stop_sequence": return { kind: "stop" };
		case "tool_use": return { kind: "tool-calls" };
		case "max_tokens": return { kind: "max-tokens" };
		default: return malformed(`unsupported stop reason ${String(raw)}`);
	}
}
/** Translate decoded SSE data into the Harness stream protocol.
* @param events - framed, decoded provider events in arrival order.
* @param model - requested model id stored in durable replay state.
* @returns blocks, one final usage value, and exactly one terminal finish.
*/
async function* translate(events, model) {
	const blocks = /* @__PURE__ */ new Map();
	const usage = {
		inputTokens: 0,
		outputTokens: 0
	};
	let started = false;
	let reason;
	for await (const event of events) {
		if (event.type === "message_start") {
			if (started) return malformed("duplicate message_start");
			updateUsage(usage, object(event.message).usage);
			started = true;
			continue;
		}
		if (![
			"content_block_start",
			"content_block_delta",
			"content_block_stop",
			"message_delta",
			"message_stop"
		].includes(String(event.type))) continue;
		if (!started) return malformed("event precedes message_start");
		if (event.type === "content_block_start") {
			const wireIndex = indexOf(event);
			if (blocks.has(wireIndex) || reason !== void 0) return malformed("block starts after settlement or repeats an index");
			const block = startBlock(event, blocks.size);
			blocks.set(wireIndex, block);
			yield {
				type: "block-start",
				index: block.index,
				blockType: block.content.type
			};
			if (block.content.type === "text" || block.content.type === "reasoning") {
				if (block.content.text) yield {
					type: block.content.type === "text" ? "text-delta" : "reasoning-delta",
					index: block.index,
					text: block.content.text
				};
			} else yield {
				type: "tool-call-delta",
				index: block.index,
				id: block.content.id,
				name: block.content.name,
				argumentsDelta: ""
			};
		} else if (event.type === "content_block_delta" || event.type === "content_block_stop") {
			const block = blocks.get(indexOf(event));
			if (block === void 0 || block.closed) return malformed("delta/stop without an open block");
			if (event.type === "content_block_delta") {
				const chunk = deltaChunk(block, event.delta);
				if (chunk !== void 0) yield chunk;
			} else {
				block.closed = true;
				if (block.content.type === "tool-call" && block.json.length > 0) block.content.arguments = block.json;
				yield {
					type: "block-end",
					index: block.index,
					block: { ...block.content }
				};
			}
		} else if (event.type === "message_delta") {
			const delta = object(event.delta);
			if (delta.stop_reason != null) reason = stopReason(delta.stop_reason);
			if (event.usage !== void 0) updateUsage(usage, event.usage);
		} else {
			if (reason === void 0 || [...blocks.values()].some((block) => !block.closed)) return malformed("message_stop without settled blocks and stop reason");
			if (blocks.size === 0 && reason.kind === "stop") throw new LlmError("DeepSeek Messages returned no content", "EMPTY_RESPONSE");
			if (reason.kind !== "max-tokens") for (const { content } of blocks.values()) {
				if (content.type !== "tool-call") continue;
				let parsed;
				try {
					parsed = JSON.parse(content.arguments);
				} catch (_invalidProviderToolJson) {
					return malformed("tool input is invalid JSON");
				}
				object(parsed);
			}
			usage.totalTokens = usage.inputTokens + usage.outputTokens + (usage.cacheReadTokens ?? 0) + (usage.cacheWriteTokens ?? 0);
			yield {
				type: "usage",
				usage
			};
			yield {
				type: "finish",
				reason,
				replayState: replayState(model, [...blocks.values()].map((block) => block.replay))
			};
			return;
		}
	}
	throw new LlmError("DeepSeek Messages stream ended before message_stop", "STREAM_CLOSED");
}
//#endregion
