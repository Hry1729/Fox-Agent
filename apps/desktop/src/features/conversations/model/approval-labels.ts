import type { ApprovalRecord, ToolCallRecord } from './types'

/**
 * Wording for an approval's state. Approvals no longer expire on their own; rows
 * written by older builds can still be `expired` and read "已过期", never
 * "用户拒绝".
 */
export const APPROVAL_STATUS_LABELS: Record<ApprovalRecord['status'], string> = {
  pending: '等你处理',
  approved: '已批准',
  denied: '用户拒绝',
  cancelled: '已取消',
  expired: '已过期',
}

export function approvalStatusLabel(status: string): string {
  return (APPROVAL_STATUS_LABELS as Record<string, string>)[status] ?? status
}

/**
 * D1-6/C02: the irreversible Host execution barrier merged into an approval's
 * decision when a capability was revoked (or its connector disabled/deleted).
 * Parsed at the unknown boundary — only an object with a non-empty code/message
 * and a positive timestamp qualifies, so a corrupt or foreign value can never
 * masquerade as one. Rendered as plain text, never innerHTML.
 */
export interface ApprovalInvalidation {
  code: string
  message: string
  at: number
}

export function approvalInvalidation(decision: unknown): ApprovalInvalidation | null {
  if (!decision || typeof decision !== 'object') return null
  const raw = (decision as Record<string, unknown>).invalidation
  if (!raw || typeof raw !== 'object') return null
  const { code, message, at } = raw as Record<string, unknown>
  if (typeof code !== 'string' || typeof message !== 'string' || typeof at !== 'number') return null
  const trimmedCode = code.trim()
  const trimmedMessage = message.trim()
  if (!trimmedCode || !trimmedMessage || !(at > 0)) return null
  return { code: trimmedCode, message: trimmedMessage, at }
}

/** Main status of a proven-unexecuted invalidated approval — never "用户拒绝". */
export const APPROVAL_INVALIDATED_TITLE = '本次操作未执行，旧的批准不会恢复'
export const APPROVAL_INVALIDATED_HINT = '重新授权后请重新发起'

export function approvalInvalidationNote(decision: unknown): string | null {
  const invalidation = approvalInvalidation(decision)
  if (!invalidation) return null
  return `${APPROVAL_INVALIDATED_TITLE}：${invalidation.message}。${APPROVAL_INVALIDATED_HINT}`
}

/**
 * D1-6: the invalidation barrier is monotonic. A late `approved` event emitted
 * before a recovery consumed/invalidated the record must not erase an
 * invalidation the client has already seen. When the incoming record lacks a
 * barrier but the previous one had it, keep the previous (validated) barrier on
 * the incoming record. Events only accelerate; the durable DB reload is truth.
 */
export function withMonotonicInvalidation<T extends { decision: unknown | null }>(previous: T, incoming: T): T {
  const prior = approvalInvalidation(previous.decision)
  if (!prior || approvalInvalidation(incoming.decision)) return incoming
  const base = incoming.decision && typeof incoming.decision === 'object' && !Array.isArray(incoming.decision)
    ? (incoming.decision as Record<string, unknown>)
    : {}
  return { ...incoming, decision: { ...base, invalidation: prior } }
}

/**
 * The note shown under a tool call. A tool call whose approval ended without a
 * decision (an old `expired` row, or a cancelled turn) shows that outcome, even
 * when an older build stored "用户拒绝" as the tool's error message.
 */
/** Reasons the Host records when a turn ended (closed, failed, interrupted) while an approval was open. */
const TURN_ENDED_REASONS = new Set(['run_interrupted', 'run_failed', 'run_completed', 'new_run_started', 'conversation_closed'])

function decisionReason(decision: unknown): string | null {
  if (!decision || typeof decision !== 'object') return null
  const reason = (decision as Record<string, unknown>).reason
  return typeof reason === 'string' ? reason : null
}

export function toolCallApprovalNote(
  tool: Pick<ToolCallRecord, 'id' | 'errorMessage'>,
  approvals: ReadonlyArray<Pick<ApprovalRecord, 'toolCallId' | 'status'> & { decision?: unknown }> | null | undefined,
): string | null {
  const approval = approvals?.find((item) => item.toolCallId === tool.id)
  // D1-6: a persisted invalidation outranks every other state — the operation
  // provably never ran, so the card must say so with its specific reason rather
  // than "已批准", "用户拒绝" or a bare timeout.
  const invalidationNote = approvalInvalidationNote(approval?.decision)
  if (invalidationNote) return invalidationNote
  if (approval?.status === 'expired') {
    // An `expired` row closed by the end of its turn (conversation closed, run
    // interrupted or failed) is not a timeout.
    return TURN_ENDED_REASONS.has(decisionReason(approval.decision) ?? '') ? '已结束（本轮已结束）' : approvalStatusLabel('expired')
  }
  if (approval?.status === 'cancelled') return approvalStatusLabel('cancelled')
  return tool.errorMessage
}
