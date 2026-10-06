import type { ApprovalRecord, ConversationDetail, RunEventRecord } from './types'
import { snapshotForRun } from './kernel-snapshot'

const terminalOwners = new Set(['completed', 'failed', 'cancelled', 'interrupted', 'budget_exhausted', 'approval_expired'])
const terminalTools = new Set(['completed', 'failed', 'cancelled', 'interrupted', 'denied', 'expired'])

/** An old ticket cannot block the composer after its owning Run or tool settles. */
export function pendingRuntimeApprovals(detail: ConversationDetail | null): ApprovalRecord[] {
  if (!detail) return []
  const runs = new Map((detail.runs ?? []).map(run => [run.id, run.status as string]))
  for (const child of detail.childRuns ?? []) runs.set(child.childRunId, child.status)
  if (detail.lastRun) runs.set(detail.lastRun.id, snapshotForRun(detail)?.state ?? detail.lastRun.status)
  const tools = new Map(detail.toolCalls.map(tool => [tool.id, tool]))
  return detail.approvals.filter(approval => approval.status === 'pending'
    && !terminalOwners.has(runs.get(approval.runId) ?? '')
    && !terminalTools.has(tools.get(approval.toolCallId)?.status ?? ''))
}

export type RuntimeQuestionOption = {
  label: string
  value: string
}

export type RuntimeQuestion = {
  questionId: string
  question: string
  options: RuntimeQuestionOption[]
  multiSelect: boolean
  allowOther: boolean
}

export type RuntimeQuestionRequest = {
  runId: string
  questions: RuntimeQuestion[]
}

function stringField(item: Record<string, unknown>, snakeCase: string, camelCase: string) {
  const value = item[snakeCase] ?? item[camelCase]
  return typeof value === 'string' ? value : undefined
}

function booleanField(item: Record<string, unknown>, snakeCase: string, camelCase: string) {
  const value = item[snakeCase] ?? item[camelCase]
  return typeof value === 'boolean' ? value : undefined
}

function parseQuestions(event: RunEventRecord['event']): RuntimeQuestion[] {
  if (!Array.isArray(event.questions)) return []
  return event.questions.flatMap((raw, index): RuntimeQuestion[] => {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return []
    const item = raw as Record<string, unknown>
    const question = typeof item.question === 'string' ? item.question.trim() : ''
    if (!question) return []
    const options = Array.isArray(item.options)
      ? item.options.flatMap((rawOption): RuntimeQuestionOption[] => {
          if (typeof rawOption === 'string') return [{ label: rawOption, value: rawOption }]
          if (!rawOption || typeof rawOption !== 'object' || Array.isArray(rawOption)) return []
          const option = rawOption as Record<string, unknown>
          const label = typeof option.label === 'string' ? option.label.trim() : ''
          const value = typeof option.value === 'string' ? option.value : label
          return label && value ? [{ label, value }] : []
        })
      : []
    return [{
      questionId: stringField(item, 'question_id', 'questionId') ?? `q-${index + 1}`,
      question,
      options,
      multiSelect: booleanField(item, 'multi_select', 'multiSelect') === true,
      allowOther: booleanField(item, 'allow_other', 'allowOther') !== false,
    }]
  })
}

function eventOrder(left: RunEventRecord, right: RunEventRecord) {
  return left.createdAt - right.createdAt || left.seq - right.seq
}

function requestIsClosed(events: RunEventRecord[], requested: RunEventRecord) {
  return events.some((item) => {
    if (item.runId !== requested.runId || item.seq <= requested.seq) return false
    if (item.eventType === 'user.question.responded') return true
    if (item.eventType === 'run.failed' || item.eventType === 'run.cancelled' || item.eventType === 'run.interrupted') return true
    return item.eventType === 'run.completed' && item.event.completionReason !== 'awaiting_user'
  })
}

function hasLegacyResponse(detail: ConversationDetail, requested: RunEventRecord) {
  const questions = parseQuestions(requested.event)
  if (!questions.length) return false
  return detail.messages.some((message) => (
    message.role === 'user'
    && message.createdAt > requested.createdAt
    && message.runId !== requested.runId
    && questions.every((question) => (
      message.content.includes(`${question.question}：`)
      || message.content.includes(`${question.question}:`)
    ))
  ))
}

export function pendingRuntimeQuestion(detail: ConversationDetail | null): RuntimeQuestionRequest | null {
  if (!detail) return null
  const events = [...detail.runtimeEvents].sort(eventOrder)
  const requests = events.filter((item) => item.eventType === 'user.question.requested').reverse()
  for (const requested of requests) {
    if (requestIsClosed(events, requested) || hasLegacyResponse(detail, requested)) continue
    const questions = parseQuestions(requested.event)
    if (questions.length) return { runId: requested.runId, questions }
  }
  return null
}
