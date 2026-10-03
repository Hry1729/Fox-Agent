import type { RunEventRecord } from '@/features/conversations/model/types'

/** The four event types that end a Run. Shared so every projection agrees. */
export const TERMINAL_RUN_EVENT_TYPES = new Set([
  'run.completed',
  'run.failed',
  'run.cancelled',
  'run.interrupted',
])

const terminalRunEvents = TERMINAL_RUN_EVENT_TYPES

/** Use persisted lifecycle events only. Message timestamps do not measure a run. */
export function runElapsedBounds(events: readonly RunEventRecord[]) {
  let started: RunEventRecord | undefined
  for (const item of events) {
    if (item.eventType === 'run.started' && Number.isFinite(item.createdAt)
      && (!started || item.seq < started.seq)) started = item
  }
  if (!started) return { startedAt: null, endedAt: null }
  let ended: RunEventRecord | undefined
  for (const item of events) {
    if (terminalRunEvents.has(item.eventType) && item.seq >= started.seq
      && Number.isFinite(item.createdAt) && item.createdAt >= started.createdAt
      && (!ended || item.seq > ended.seq)) ended = item
  }
  return { startedAt: started.createdAt, endedAt: ended?.createdAt ?? null }
}

export function formatRunElapsed(milliseconds: number) {
  const seconds = Math.floor(Math.max(0, milliseconds) / 1000)
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const remaining = seconds % 60
  if (hours > 0) return `${hours}时${minutes}分${remaining}秒`
  if (minutes > 0) return `${minutes}分${remaining}秒`
  return `${remaining}秒`
}
