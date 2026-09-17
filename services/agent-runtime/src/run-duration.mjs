// Ordinary conversations have no whole-task timer. Explicit delegated budgets
// and older frozen bindings keep the duration limit they were authorized with.
export function runDurationMs(payload) {
  const limits = []
  const explicit = payload?.runBudget?.maxDurationMs
  if (Number.isSafeInteger(explicit) && explicit > 0) limits.push(Math.min(explicit, 86_400_000))
  const frozen = payload?.controlBinding?.budgets
  if (frozen && frozen.runExecutionLimited !== false) limits.push(frozen.runExecutionMs)
  return limits.length ? Math.min(...limits) : null
}

export function armRunDurationTimer(durationMs, onTimeout) {
  return durationMs === null ? null : setTimeout(onTimeout, durationMs)
}
