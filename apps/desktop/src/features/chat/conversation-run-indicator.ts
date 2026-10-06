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
import { TERMINAL_RUN_EVENT_TYPES } from './turn-process-timing'

export type ConversationRunIndicator = 'queued' | 'running' | 'approval' | 'reply' | 'completed'

export interface ConversationRunOverlayEntry {
  /**
   * `completed` records success; `null` settles other terminal outcomes. Both
   * let a terminal event beat
   * a list snapshot that was fetched before the run finished, so the indicator
   * always stops on time.
   */
  indicator: ConversationRunIndicator | null
  updatedAt: number
  /** The run that produced this event, so a terminal event cannot hide a later run. */
  runId?: string
}

export type ConversationRunOverlay = ReadonlyMap<string, ConversationRunOverlayEntry>

export const EMPTY_CONVERSATION_RUN_OVERLAY: ConversationRunOverlay = new Map()

const TERMINAL_EVENT_TYPES = TERMINAL_RUN_EVENT_TYPES

/** Run statuses that still mean the conversation is busy. */
const QUEUED_RUN_STATUSES = new Set(['queued'])
const APPROVAL_RUN_STATUSES = new Set(['awaiting_confirmation'])

/** The indicator a sidebar row shows from its own persisted Run state. */
export function conversationRunIndicator(
  summary: Pick<ConversationSummary, 'activeRunId' | 'activeRunStatus' | 'awaitingApproval' | 'lastRunId' | 'lastRunStatus' | 'awaitingReply'>,
): ConversationRunIndicator | null {
  if (summary.awaitingReply) return 'reply'
  if (!summary.activeRunId) return summary.lastRunStatus === 'completed' ? 'completed' : null
  const status = summary.activeRunStatus ?? 'running'
  if (summary.awaitingApproval || APPROVAL_RUN_STATUSES.has(status)) return 'approval'
  if (QUEUED_RUN_STATUSES.has(status)) return 'queued'
  return 'running'
}

/**
 * Live events for one conversation. Events never leak into another conversation:
 * the key is the event's own `conversationId`, and a terminal event only clears
 * that conversation's entry; successful completion retains the green check.
 */
export function applyRunEventToOverlay(
  overlay: ConversationRunOverlay,
  notification: Pick<RuntimeEventNotification, 'conversationId' | 'runId' | 'event'>,
  now: number,
): ConversationRunOverlay {
  const conversationId = notification.conversationId
  if (!conversationId) return overlay
  const eventType = notification.event?.type
  if (typeof eventType !== 'string') return overlay
  const current = overlay.get(conversationId)
  // Waiting for approval outranks "running" until the approval is resolved or the
  // run ends; the run keeps emitting unrelated events while the user decides.
  const sameRun = current?.runId === notification.runId
  if (sameRun && (current?.indicator === null || current?.indicator === 'completed')
    && !TERMINAL_EVENT_TYPES.has(eventType) && eventType !== 'user.question.requested') return overlay
  const indicator: ConversationRunIndicator | null = eventType === 'user.question.requested'
    ? 'reply'
    : eventType === 'user.question.responded'
      ? 'running'
      : TERMINAL_EVENT_TYPES.has(eventType)
        ? eventType === 'run.completed'
          ? notification.event.completionReason === 'awaiting_user' ? 'reply' : 'completed'
          : null
        : eventType === 'run.waiting_approval' || (sameRun && current?.indicator === 'approval')
          ? 'approval'
          : sameRun && current?.indicator === 'reply' ? 'reply' : 'running'
  // Unchanged state returns the same map: a long run emits thousands of deltas and
  // must not re-render the sidebar for each of them.
  if (current?.indicator === indicator && current.runId === notification.runId) return overlay
  const next = new Map(overlay)
  next.set(conversationId, { indicator, updatedAt: now, runId: notification.runId })
  return next
}

/**
 * An approval asks for the user's attention, so it outranks "running". A resolved
 * approval only clears that marker: whether the conversation is running again is
 * decided by the Run state and by the events that follow.
 */
export function applyApprovalToOverlay(
  overlay: ConversationRunOverlay,
  approval: { conversationId: string; status: string; runId?: string },
  now: number,
): ConversationRunOverlay {
  if (!approval.conversationId) return overlay
  const current = overlay.get(approval.conversationId)
  if (approval.runId && current?.runId === approval.runId
    && (current.indicator === null || current.indicator === 'completed')) return overlay
  if (approval.status !== 'pending') {
    if (approval.runId && current?.runId && approval.runId !== current.runId) return overlay
    if (current?.indicator !== 'approval') return overlay
    const next = new Map(overlay)
    next.delete(approval.conversationId)
    return next
  }
  if (current?.indicator === 'approval' && (!approval.runId || approval.runId === current.runId)) return overlay
  const next = new Map(overlay)
  next.set(approval.conversationId, { indicator: 'approval', updatedAt: now, runId: approval.runId ?? current?.runId })
  return next
}

/**
 * Drops entries that the newest list snapshot already accounts for, and entries
 * whose conversation is gone. Live entries younger than the snapshot survive, so
 * a run that starts while the snapshot is in flight is not lost.
 */
export function pruneRunOverlay(
  overlay: ConversationRunOverlay,
  conversations: readonly Pick<ConversationSummary, 'id' | 'activeRunId'>[],
  snapshotAt: number,
): ConversationRunOverlay {
  const activeRunIds = new Map(conversations.map(({ id, activeRunId }) => [id, activeRunId]))
  let next: Map<string, ConversationRunOverlayEntry> | null = null
  for (const [conversationId, entry] of overlay) {
    const known = activeRunIds.has(conversationId)
    // A list read can lag the terminal event. Keep the tombstone while that same
    // run is still listed as active; a different run must remain visible.
    const terminal = entry.indicator === null || entry.indicator === 'completed'
    const staleTerminalRun = terminal && entry.runId
      && activeRunIds.get(conversationId) === entry.runId
    const superseded = terminal ? entry.updatedAt <= snapshotAt : entry.updatedAt < snapshotAt
    const replacedTerminalRun = terminal && entry.runId
      && activeRunIds.get(conversationId) && activeRunIds.get(conversationId) !== entry.runId
    if (known && !replacedTerminalRun && (staleTerminalRun || !superseded)) continue
    if (next === null) next = new Map(overlay)
    next.delete(conversationId)
  }
  return next ?? overlay
}

/**
 * Effective indicator per conversation: live evidence newer than the newest list
 * snapshot wins, otherwise the authoritative snapshot does.
 */
export function conversationRunActivity(
  conversation: ConversationSummary,
  overlay: ConversationRunOverlay,
  snapshotAt: number,
) {
  const live = overlay.get(conversation.id)
  const terminal = live?.indicator === null || live?.indicator === 'completed'
  const terminalForListedRun = terminal && live?.runId && live.runId === conversation.activeRunId
  const listedRun = conversation.activeRunId ?? conversation.lastRunId ?? null
  if (live?.indicator === 'approval' && live.runId && !conversation.activeRunId
    && live.runId === conversation.lastRunId && ['completed', 'failed', 'cancelled', 'interrupted'].includes(conversation.lastRunStatus ?? '')) {
    return { indicator: conversationRunIndicator(conversation), runId: listedRun }
  }
  const terminalForOtherRun = terminal && live?.runId && listedRun && live.runId !== listedRun
  const useLive = live && !terminalForOtherRun && (live.updatedAt > snapshotAt || terminalForListedRun)
  return {
    indicator: useLive ? live.indicator : conversationRunIndicator(conversation),
    runId: useLive ? live.runId ?? listedRun : listedRun,
  }
}

export function conversationRunIndicators(
  conversations: readonly ConversationSummary[],
  overlay: ConversationRunOverlay,
  snapshotAt: number,
): ReadonlyMap<string, ConversationRunIndicator> {
  const indicators = new Map<string, ConversationRunIndicator>()
  for (const conversation of conversations) {
    const { indicator } = conversationRunActivity(conversation, overlay, snapshotAt)
    if (indicator) indicators.set(conversation.id, indicator)
  }
  return indicators
}
