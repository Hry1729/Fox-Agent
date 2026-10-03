/**
 * Per-conversation activity for the sidebar.
 *
 * The sidebar used to animate whichever conversation was open, so switching away
 * from a running conversation hid its indicator. Activity is now derived from each
 * conversation's own Run:
 *
 * - the authoritative conversation list carries the conversation's own
 *   non-terminal Run (`activeRunId`/`activeRunStatus`/`awaitingApproval`), which
 *   survives a reload or a reconnect;
 * - live runtime-event notifications refine it between list refreshes without
 *   refreshing the whole list on every delta.
 */

import type { ConversationSummary, RuntimeEventNotification } from '@/features/conversations/model/types'

export type ConversationRunIndicator = 'queued' | 'running' | 'approval'

export interface ConversationRunOverlayEntry {
  /**
   * `null` records "this conversation just ended". It lets a terminal event beat
   * a list snapshot that was fetched before the run finished, so the indicator
   * always stops on time.
   */
  indicator: ConversationRunIndicator | null
  updatedAt: number
}

export type ConversationRunOverlay = ReadonlyMap<string, ConversationRunOverlayEntry>

export const EMPTY_CONVERSATION_RUN_OVERLAY: ConversationRunOverlay = new Map()

const TERMINAL_EVENT_TYPES = new Set([
  'run.completed',
  'run.cancelled',
  'run.failed',
  'run.interrupted',
])

/** Run statuses that still mean the conversation is busy. */
const QUEUED_RUN_STATUSES = new Set(['queued'])
const APPROVAL_RUN_STATUSES = new Set(['awaiting_confirmation'])

/** The indicator a sidebar row shows from its own persisted Run state. */
export function conversationRunIndicator(
  summary: Pick<ConversationSummary, 'activeRunId' | 'activeRunStatus' | 'awaitingApproval'>,
): ConversationRunIndicator | null {
  if (!summary.activeRunId) return null
  const status = summary.activeRunStatus ?? 'running'
  if (summary.awaitingApproval || APPROVAL_RUN_STATUSES.has(status)) return 'approval'
  if (QUEUED_RUN_STATUSES.has(status)) return 'queued'
  return 'running'
}

/**
 * Live events for one conversation. Events never leak into another conversation:
 * the key is the event's own `conversationId`, and a terminal event only clears
 * that conversation's entry.
 */
export function applyRunEventToOverlay(
  overlay: ConversationRunOverlay,
  notification: Pick<RuntimeEventNotification, 'conversationId' | 'event'>,
  now: number,
): ConversationRunOverlay {
  const conversationId = notification.conversationId
  if (!conversationId) return overlay
  const eventType = notification.event?.type
  if (typeof eventType !== 'string') return overlay
  const current = overlay.get(conversationId)
  // Waiting for approval outranks "running" until the approval is resolved or the
  // run ends; the run keeps emitting unrelated events while the user decides.
  const indicator: ConversationRunIndicator | null = TERMINAL_EVENT_TYPES.has(eventType)
    ? null
    : eventType === 'run.waiting_approval' || current?.indicator === 'approval'
      ? 'approval'
      : 'running'
  // Unchanged state returns the same map: a long run emits thousands of deltas and
  // must not re-render the sidebar for each of them.
  if (current?.indicator === indicator) return overlay
  const next = new Map(overlay)
  next.set(conversationId, { indicator, updatedAt: now })
  return next
}

/**
 * An approval asks for the user's attention, so it outranks "running". A resolved
 * approval only clears that marker: whether the conversation is running again is
 * decided by the Run state and by the events that follow.
 */
export function applyApprovalToOverlay(
  overlay: ConversationRunOverlay,
  approval: { conversationId: string; status: string },
  now: number,
): ConversationRunOverlay {
  if (!approval.conversationId) return overlay
  const current = overlay.get(approval.conversationId)
  if (approval.status !== 'pending') {
    if (current?.indicator !== 'approval') return overlay
    const next = new Map(overlay)
    next.delete(approval.conversationId)
    return next
  }
  if (current?.indicator === 'approval') return overlay
  const next = new Map(overlay)
  next.set(approval.conversationId, { indicator: 'approval', updatedAt: now })
  return next
}

/**
 * Drops entries that the newest list snapshot already accounts for, and entries
 * whose conversation is gone. Live entries younger than the snapshot survive, so
 * a run that starts while the snapshot is in flight is not lost.
 */
export function pruneRunOverlay(
  overlay: ConversationRunOverlay,
  conversationIds: ReadonlySet<string>,
  snapshotAt: number,
): ConversationRunOverlay {
  let next: Map<string, ConversationRunOverlayEntry> | null = null
  for (const [conversationId, entry] of overlay) {
    const known = conversationIds.has(conversationId)
    const superseded = entry.indicator === null ? entry.updatedAt <= snapshotAt : entry.updatedAt < snapshotAt
    if (known && !superseded) continue
    if (next === null) next = new Map(overlay)
    next.delete(conversationId)
  }
  return next ?? overlay
}

/**
 * Effective indicator per conversation: live evidence newer than the newest list
 * snapshot wins, otherwise the authoritative snapshot does.
 */
export function conversationRunIndicators(
  conversations: readonly ConversationSummary[],
  overlay: ConversationRunOverlay,
  snapshotAt: number,
): ReadonlyMap<string, ConversationRunIndicator> {
  const indicators = new Map<string, ConversationRunIndicator>()
  for (const conversation of conversations) {
    const live = overlay.get(conversation.id)
    const indicator = live && live.updatedAt > snapshotAt ? live.indicator : conversationRunIndicator(conversation)
    if (indicator) indicators.set(conversation.id, indicator)
  }
  return indicators
}
