// Ordinary conversations have no whole-task timer. Explicit delegated budgets
// and older frozen bindings keep the duration limit they were authorized with.
//
// A limit is only armed when it is a real positive number. A frozen binding
// that names `runExecutionLimited` but omits `runExecutionMs` (or carries a
// non-numeric value) used to reach `Math.min()` as `undefined`, so the budget
// became `NaN`, `setTimeout(NaN)` fired immediately and every such Run failed
// with a bogus `runtime.duration_budget_exceeded` before its first token.
const MAX_DURATION_MS = 86_400_000

function durationLimit(value) {
  return Number.isSafeInteger(value) && value > 0 ? Math.min(value, MAX_DURATION_MS) : null
}

export function runDurationMs(payload) {
  const limits = []
  const explicit = durationLimit(payload?.runBudget?.maxDurationMs)
  if (explicit !== null) limits.push(explicit)
  const frozen = payload?.controlBinding?.budgets
  if (frozen && frozen.runExecutionLimited !== false) {
    const frozenLimit = durationLimit(frozen.runExecutionMs)
    if (frozenLimit !== null) limits.push(frozenLimit)
  }
  return limits.length ? Math.min(...limits) : null
}

export function armRunDurationTimer(durationMs, onTimeout) {
  if (!Number.isFinite(durationMs) || durationMs <= 0) return null
  return setTimeout(onTimeout, durationMs)
}
