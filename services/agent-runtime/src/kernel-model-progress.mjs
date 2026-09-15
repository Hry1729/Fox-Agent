// Per-round model output progress for Kernel workers (live loop and
// single-round resume share this contract).
//
// Frozen Host budgets (controlBinding.budgets, camelCase):
//   modelRequestMs       whole-round backstop, slow-but-progressing output
//                        included; the Host controller enforces the same bound.
//   modelFirstResponseMs no output at all within this long fails the round.
//   modelIdleMs          no text/thinking/tool-parameter progress within this
//                        long fails the round even below the whole-round bound.
//
// Text, thinking, AND tool-parameter bytes all count as progress: long Office
// JSON generated as tool arguments must not look like a stalled stream.
// Only byte counts leave this module; no content, credentials, or model input
// is ever recorded, so telemetry is safe to persist in failure evidence.

const positiveInt = (value, fallback) =>
  Number.isFinite(Number(value)) && Number(value) > 0 ? Math.floor(Number(value)) : fallback

export function createRoundProgress({ budgets, now = () => performance.now() } = {}) {
  const total = positiveInt(budgets?.modelRequestMs, 120_000)
  const first = Math.min(positiveInt(budgets?.modelFirstResponseMs, 60_000), total)
  const idle = Math.min(positiveInt(budgets?.modelIdleMs, 120_000), total)
  const startedAt = now()
  let firstAt = 0
  let lastAt = 0
  let textBytes = 0
  let reasoningBytes = 0
  let toolParamBytes = 0

  const snapshot = () => {
    const at = firstAt || lastAt || startedAt
    const elapsed = Math.max(0, Math.floor(at - startedAt))
    return {
      elapsedMs: Math.max(0, Math.floor(now() - startedAt)),
      idleElapsedMs: Math.max(0, Math.floor(now() - (lastAt || startedAt))),
      firstResponseMs: firstAt ? Math.max(0, Math.floor(firstAt - startedAt)) : null,
      textBytes, reasoningBytes, toolParamBytes,
      totalBytes: textBytes + reasoningBytes + toolParamBytes,
      bounds: { total, first, idle },
      _elapsed: elapsed,
    }
  }

  // Record one observation of the current assistant message size. Any growth
  // in any channel marks progress. Returns true when this observation is new
  // progress (callers use it to throttle progress previews).
  const note = ({ text = '', reasoning = '', toolParamBytes: tools = 0 } = {}) => {
    const textLen = Buffer.byteLength(String(text), 'utf8')
    const reasoningLen = Buffer.byteLength(String(reasoning), 'utf8')
    const toolLen = Number.isFinite(Number(tools)) && Number(tools) > 0 ? Math.floor(Number(tools)) : 0
    const at = now()
    let progressed = false
    if (textLen !== textBytes || reasoningLen !== reasoningBytes || toolLen !== toolParamBytes) {
      progressed = true
      if (!firstAt && (textLen > 0 || reasoningLen > 0 || toolLen > 0)) firstAt = at
      textBytes = textLen
      reasoningBytes = reasoningLen
      toolParamBytes = toolLen
      lastAt = at
    }
    return progressed
  }

  // Classify the round: ok, or which bound is breached. Pure function of the
  // frozen budgets and observed progress; Host controller applies the same
  // semantics from its own anchors.
  const check = (at = now()) => {
    const elapsed = at - startedAt
    const idleElapsed = at - (lastAt || startedAt)
    const responded = firstAt !== 0
    if (!responded && elapsed >= first) return 'first_response'
    if (responded && idleElapsed >= idle) return 'idle'
    if (elapsed >= total) return 'total'
    return 'ok'
  }

  const telemetry = () => {
    const state = snapshot()
    return {
      elapsedMs: state.elapsedMs,
      idleElapsedMs: state.idleElapsedMs,
      firstResponseMs: state.firstResponseMs,
      textBytes: state.textBytes,
      reasoningBytes: state.reasoningBytes,
      toolParamBytes: state.toolParamBytes,
    }
  }

  return { bounds: { total, first, idle }, startedAt, note, check, telemetry,
    totalBytes: () => textBytes + reasoningBytes + toolParamBytes }
}

// Byte size of tool-call arguments in one assistant content array. Counts the
// serialized arguments the model actually produced this round.
export function toolParamBytesOf(content) {
  if (!Array.isArray(content)) return 0
  let bytes = 0
  for (const block of content) {
    if (block?.type === 'toolCall' && block.arguments !== undefined) {
      try {
        bytes += Buffer.byteLength(JSON.stringify(block.arguments), 'utf8')
      } catch {
        bytes += 0
      }
    }
  }
  return bytes
}
