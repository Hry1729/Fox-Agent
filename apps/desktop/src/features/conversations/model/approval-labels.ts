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
  if (approval?.status === 'expired') {
    // An `expired` row closed by the end of its turn (conversation closed, run
    // interrupted or failed) is not a timeout.
    return TURN_ENDED_REASONS.has(decisionReason(approval.decision) ?? '') ? '已结束（本轮已结束）' : approvalStatusLabel('expired')
  }
  if (approval?.status === 'cancelled') return approvalStatusLabel('cancelled')
  return tool.errorMessage
}
