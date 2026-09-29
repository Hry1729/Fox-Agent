import type { ApprovalDecision, ApprovalRecord, ApprovalRequest } from './types'
import { wholeFileReplacementBinding } from './approval-presentation'

const DECISION_ORDER: readonly ApprovalDecision[] = [
  'deny',
  'allow_once',
  'allow_conversation',
]

const DECISIONS = new Set<ApprovalDecision>(DECISION_ORDER)

type ApprovalCategoryState =
  | 'legacy_tool_execution'
  | 'tool_execution'
  | 'task_repair_budget_override'
  | 'unknown'

export interface RepairOverrideApprovalDetails {
  rootCause: string
  findingIds: readonly string[]
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function hasOwn(value: Record<string, unknown>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(value, key)
}

function categoryState(request: Record<string, unknown>): ApprovalCategoryState {
  if (!hasOwn(request, 'category')) return 'legacy_tool_execution'
  if (request.category === 'tool_execution') return 'tool_execution'
  if (request.category === 'task_repair_budget_override') return 'task_repair_budget_override'
  return 'unknown'
}

function explicitDecisions(request: Record<string, unknown>): Set<ApprovalDecision> | null | undefined {
  if (!hasOwn(request, 'availableDecisions')) return undefined
  const value = request.availableDecisions
  if (!Array.isArray(value) || value.length === 0) return null

  const decisions = new Set<ApprovalDecision>()
  for (const item of value) {
    if (typeof item !== 'string' || !DECISIONS.has(item as ApprovalDecision)) return null
    const decision = item as ApprovalDecision
    if (decisions.has(decision)) return null
    decisions.add(decision)
  }
  return decisions
}

export function repairOverrideApprovalDetails(request: unknown): RepairOverrideApprovalDetails | null {
  if (!isRecord(request) || categoryState(request) !== 'task_repair_budget_override') return null
  const args = request.arguments
  if (!isRecord(args)) return null
  if (!hasOwn(args, 'rootCause') || !hasOwn(args, 'findingIds')) return null

  const rootCause = typeof args.rootCause === 'string' ? args.rootCause.trim() : ''
  if (!rootCause || rootCause.length > 4_000) return null
  if (!Array.isArray(args.findingIds) || args.findingIds.length < 1 || args.findingIds.length > 32) return null

  const findingIds: string[] = []
  const seen = new Set<string>()
  for (const item of args.findingIds) {
    const findingId = typeof item === 'string' ? item.trim() : ''
    if (!findingId || findingId.length > 200 || seen.has(findingId)) return null
    seen.add(findingId)
    findingIds.push(findingId)
  }
  return { rootCause, findingIds }
}

/**
 * Turns the Host's untrusted decision declaration into the only decisions the UI may send.
 * Missing declarations retain the legacy three-button contract only for ordinary tool approvals.
 */
function baseApprovalDecisions(request: unknown): readonly ApprovalDecision[] {
  if (!isRecord(request)) return ['deny']
  const category = categoryState(request)
  const declared = explicitDecisions(request)

  if (declared === undefined) {
    return category === 'legacy_tool_execution' || category === 'tool_execution'
      ? DECISION_ORDER
      : ['deny']
  }
  if (declared === null || category === 'unknown') return ['deny']
  if (category === 'task_repair_budget_override' && !repairOverrideApprovalDetails(request)) return ['deny']

  const categoryLimit = category === 'task_repair_budget_override'
    ? new Set<ApprovalDecision>(['deny', 'allow_once'])
    : DECISIONS

  // Deny is always available as the safe exit even if the Host omitted it.
  return DECISION_ORDER.filter((decision) => (
    decision === 'deny' || (declared.has(decision) && categoryLimit.has(decision))
  ))
}

export function allowedApprovalDecisions(request: unknown): readonly ApprovalDecision[] {
  const decisions = baseApprovalDecisions(request)
  if (!isRecord(request) || !hasOwn(request, 'wholeFileReplacement')) return decisions
  // Kernel stores this purpose-specific declaration inside the binding. Never
  // offer conversation-wide permission for a one-dispatch replacement ticket.
  if (!wholeFileReplacementBinding(request) || !isRecord(request.wholeFileReplacement)) return ['deny']
  const nested = explicitDecisions(request.wholeFileReplacement)
  if (nested === null) return ['deny']
  return decisions.filter(decision => decision === 'deny'
    || decision === 'allow_once' && (nested === undefined || nested.has(decision)))
}

export function isApprovalDecisionAllowed(request: unknown, decision: ApprovalDecision): boolean {
  return allowedApprovalDecisions(request).includes(decision)
}

export async function resolveAllowedApprovalDecision(
  approval: Pick<ApprovalRecord, 'id' | 'request'>,
  decision: ApprovalDecision,
  resolver: (approvalId: string, decision: ApprovalDecision) => void | Promise<boolean>,
): Promise<boolean> {
  if (!isApprovalDecisionAllowed(approval.request, decision)) return false
  return await resolver(approval.id, decision) !== false
}

export function isRepairOverrideApproval(request: ApprovalRequest): boolean {
  return isRecord(request) && categoryState(request) === 'task_repair_budget_override'
}
