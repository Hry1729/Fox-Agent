export type RunBudgetTier = 'short' | 'standard' | 'long' | 'custom'
export interface RunBudgetSelection { tier: RunBudgetTier; customExecutionMs?: number }
export function normalizeRunBudget(value: unknown): RunBudgetSelection {
  const item = value as Partial<RunBudgetSelection> | null
  if (item?.tier === 'short' || item?.tier === 'long' || item?.tier === 'standard') return { tier: item.tier }
  if (item?.tier === 'custom' && Number.isSafeInteger(item.customExecutionMs) && item.customExecutionMs! > 0 && item.customExecutionMs! <= 86400000) return { tier: 'custom', customExecutionMs: item.customExecutionMs }
  return { tier: 'standard' }
}
export function readRunBudget(conversationId: string): RunBudgetSelection {
  try { return normalizeRunBudget(JSON.parse(localStorage.getItem(`fox.run-budget.${conversationId}`) ?? 'null')) } catch { return { tier: 'standard' } }
}
export function saveRunBudget(conversationId: string, value: RunBudgetSelection) {
  localStorage.setItem(`fox.run-budget.${conversationId}`, JSON.stringify(normalizeRunBudget(value)))
}
