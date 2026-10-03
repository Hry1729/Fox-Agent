import type { ConversationDetail, KernelRunSnapshot } from './types'

const states = new Set([
  'created', 'running', 'waiting_approval', 'retry_scheduled', 'compacting', 'cancelling',
  'completed', 'cancelled', 'failed', 'budget_exhausted', 'approval_expired',
])
const terminalStates = new Set(['completed', 'cancelled', 'failed', 'budget_exhausted', 'approval_expired'])

/** Do not interpret arbitrary Engine events as authority or compare their sequence here. */
export function snapshotForRun(detail: ConversationDetail): KernelRunSnapshot | null {
  const snapshot = detail.kernelSnapshot
  if (!snapshot || snapshot.schemaVersion !== 1 || snapshot.runId !== detail.lastRun?.id
    || !snapshot.turnId || !states.has(snapshot.state)
    || typeof snapshot.lastEventSeq !== 'string' || !/^(0|[1-9]\d{0,18})$/.test(snapshot.lastEventSeq)
    || terminalStates.has(snapshot.state) !== snapshot.terminalWritten) return null
  return snapshot
}

export function conversationRunState(detail: ConversationDetail | null): string | null {
  if (!detail) return null
  return snapshotForRun(detail)?.state ?? detail.lastRun?.status ?? null
}

export function conversationRunIsActive(detail: ConversationDetail | null): boolean {
  const state = conversationRunState(detail)
  return state !== null && ['queued', 'created', 'running', 'waiting_approval', 'retry_scheduled', 'compacting', 'cancelling'].includes(state)
}

export function conversationRunFingerprint(detail: ConversationDetail | null): string {
  const snapshot = detail && snapshotForRun(detail)
  return `${snapshot ? 'kernel' : 'legacy'}:${conversationRunState(detail)}:${snapshot?.lastEventSeq ?? detail?.lastRun?.lastSeq ?? 0}:${detail?.messages.length ?? 0}`
}

export function mergeKernelSnapshot(
  persisted: ConversationDetail, current: ConversationDetail, selectedRunId = persisted.lastRun?.id,
) {
  const nextSnapshot = snapshotForRun(persisted)
  const previousSnapshot = snapshotForRun(current)
  const next = nextSnapshot?.runId === selectedRunId ? nextSnapshot : null
  const previous = previousSnapshot?.runId === selectedRunId ? previousSnapshot : null
  if (!previous) return next
  if (!next) return previous
  // Equal revisions keep the already observed snapshot. A slow reload cannot roll back facts.
  return BigInt(next.lastEventSeq) > BigInt(previous.lastEventSeq) ? next : previous
}
