/**
 * Fox approval gate for pi (prototype, plan 6.3 / 7.3).
 *
 * pi-acp 0.0.34 does NOT map pi tool execution to ACP `session/request_permission`;
 * tools run as soon as the model calls them. pi-acp DOES map pi extension UI
 * dialogs (`ctx.ui.confirm` / `ctx.ui.select`, which pi RPC mode sends as
 * `extension_ui_request`) to ACP `session/request_permission`.
 *
 * This extension uses that path: every tool call (built-in read/bash/edit/write
 * and any extension tool) is held in pi's `tool_call` hook, which runs after
 * `tool_execution_start` and BEFORE the tool executes, until the Host answers.
 *
 *   pi tool_call hook -> ctx.ui.confirm -> RPC extension_ui_request{method:"confirm"}
 *     -> pi-acp -> ACP session/request_permission (options yes/allow_once, no/reject_once)
 *     -> Fox answers -> extension_ui_response{confirmed} -> allow or block
 *
 * Fail-closed: anything other than an explicit `confirmed: true` blocks the
 * call (reject, ACP `cancelled`, turn abort via ctx.signal, pi-acp error, or an
 * exception here). Only the original call is approved; what a bash command does
 * inside its own subprocess is not observed or gated (plan section 2).
 *
 * Load only via `pi --no-extensions -e <this file>` (see pi-gated.sh): project
 * `.pi/extensions` are not loaded in RPC mode without trust, and Fox must not
 * write the user's global ~/.pi/agent/extensions (plan 6.5).
 *
 * The confirm message is a JSON object with the marker `"fox": "tool-approval"`
 * so the Host mapper can tell a tool approval from any other extension dialog
 * and correlate it with the `tool_call` update carrying the same toolCallId.
 */

export const FOX_APPROVAL_MARKER = "tool-approval";
export const FOX_APPROVAL_TITLE_PREFIX = "Fox approval: ";

export default function foxApprovalGate(pi: any) {
	pi.on("tool_call", async (event: any, ctx: any) => {
		const toolName = String(event?.toolName ?? "unknown");
		const toolCallId = String(event?.toolCallId ?? "");
		try {
			if (!ctx?.hasUI) {
				return { block: true, reason: "Fox approval unavailable (no UI channel); blocked" };
			}
			if (ctx?.signal?.aborted) {
				return { block: true, reason: "Turn aborted before approval; blocked" };
			}
			const message = JSON.stringify({
				fox: FOX_APPROVAL_MARKER,
				v: 1,
				toolCallId,
				toolName,
				input: event?.input ?? null,
				cwd: ctx?.cwd ?? null,
			});
			// No timeout: the Host owns expiry. ctx.signal resolves the dialog to
			// `false` on abort, which blocks.
			const confirmed = await ctx.ui.confirm(`${FOX_APPROVAL_TITLE_PREFIX}${toolName}`, message, {
				signal: ctx?.signal,
			});
			if (confirmed === true) return undefined;
			return { block: true, reason: "Rejected by Fox approval (reject, cancel or abort)" };
		} catch (error) {
			return { block: true, reason: `Fox approval failed; blocked (${String((error as any)?.message ?? error)})` };
		}
	});
}
