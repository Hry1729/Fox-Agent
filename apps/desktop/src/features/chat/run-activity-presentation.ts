import type { ConversationDetail, RunEventRecord } from '@/features/conversations/model/types'
import { conversationRunState, snapshotForRun } from '@/features/conversations/model/kernel-snapshot'
import { previewBelongsToSnapshot } from '@/features/conversations/model/kernel-model-preview'
import { pendingRuntimeApprovals, pendingRuntimeQuestion } from '@/features/conversations/model/pending-interactions'
import { isCommandTool } from './composer/fox-tool-classification'
import { activeModelWaiting } from './run-status-facts'
import { TERMINAL_RUN_EVENT_TYPES } from './turn-process-timing'

export type FoxAnimation = 'idle' | 'humming' | 'loading' | 'sending' | 'receiving' | 'thinking' | 'searching' | 'radar' | 'orbit' | 'writing' | 'progress' | 'listening' | 'happy' | 'alerting'
type ToolKind = 'read' | 'directory' | 'search' | 'web' | 'command' | 'write' | 'question' | 'other'
type ToolFact = { id: string; name: string; input: Record<string, unknown>; kind: ToolKind; seq: number; completed: boolean; failed: boolean; awaitingUser: boolean; progress?: number }
const object = (value: unknown): Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
const text = (value: unknown) => typeof value === 'string' && value.trim() ? value.trim() : undefined
const terminalStates = new Set(['completed', 'failed', 'cancelled', 'interrupted', 'budget_exhausted', 'approval_expired'])

function toolKind(name: string): ToolKind {
  if (/^(ls|ls_dir|list_dir|list_directory|list_files|dir)$/.test(name)) return 'directory'
  if (/^(read|read_file|view_file|open_file|read_attachment|read_image)$/.test(name)) return 'read'
  if (/^(write_file|edit_file|write|edit|apply_patch|str_replace|patch_file)$/.test(name)) return 'write'
  if (isCommandTool(name) || /^(exec_command|bash|pwsh)$/.test(name)) return 'command'
  if (/^(web_search|web_fetch|fetch|http_get|browser_.*)$/.test(name)) return 'web'
  if (/^(grep|find|glob|search_content|search_files|query_kb|search_knowledge|read_knowledge_document|list_knowledge_bases)$/.test(name)) return 'search'
  if (/^(ask_user_question|request_user_input)$/.test(name)) return 'question'
  return 'other'
}

/** A run-scoped, replay-safe projection. A completed call cannot become active on replay. */
function runFacts(events: readonly RunEventRecord[], runId?: string | null) {
  const seen = new Set<string>()
  const ordered = events.filter(item => !runId || item.runId === runId).toSorted((a, b) => a.seq - b.seq).filter(item => {
    const key = `${item.runId}:${item.seq}:${item.eventType}`
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
  const end = ordered.findIndex(item => TERMINAL_RUN_EVENT_TYPES.has(item.eventType))
  const bounded = end < 0 ? ordered : ordered.slice(0, end + 1)
  const calls = new Map<string, ToolFact>()
  for (const item of bounded) {
    if (!['tool.started', 'tool.updated', 'tool.completed', 'tool.failed'].includes(item.eventType)) continue
    const id = text(item.event.toolCallId)
    // Without identity we cannot join a start/result or count replayed records reliably.
    if (!id) continue
    const key = `${item.runId}:${id}`
    const name = text(item.event.tool)?.toLowerCase()
    const current = calls.get(key) ?? { id: key, name: name ?? '', input: {}, kind: 'other', seq: item.seq, completed: false, failed: false, awaitingUser: false }
    if (name) current.name = name
    if (item.event.input !== undefined) current.input = object(item.event.input)
    current.kind = toolKind(current.name)
    current.seq = item.seq
    if (item.eventType === 'tool.completed' || item.eventType === 'tool.failed') {
      current.completed = true
      current.failed = item.eventType === 'tool.failed' || item.event.isError === true
      current.awaitingUser = object(item.event.result).status === 'awaiting_user'
    }
    const progress = item.event.progress
    if (typeof progress === 'number' && Number.isFinite(progress) && progress >= 0 && progress <= 100) current.progress = progress
    calls.set(key, current)
  }
  return { events: bounded, tools: [...calls.values()], terminal: end < 0 ? undefined : ordered[end] }
}

function targetIdentity(tool: ToolFact) {
  const value = text(tool.input.path) ?? text(tool.input.file_path) ?? text(tool.input.filePath) ?? text(tool.input.file)
    ?? (tool.kind === 'directory' ? text(tool.input.directory) ?? text(tool.input.dir) : undefined)
    ?? (tool.kind === 'read' ? text(tool.input.attachmentId) ?? text(tool.input.attachment_id) : undefined)
  if (!value) return undefined
  const normalized = value.replace(/\\/g, '/')
  return /^[a-z]:\//i.test(normalized) ? normalized.toLowerCase() : normalized
}

export function summarizeRunSteps(events: readonly RunEventRecord[], active: boolean, runId?: string | null): string {
  const facts = runFacts(events, runId)
  const successful = facts.tools.filter(tool => tool.completed && !tool.failed && !tool.awaitingUser)
  const fields: string[] = []
  for (const [kind, verb, unit] of [['read', '读取', '文件'], ['directory', '查看', '目录'], ['write', '修改', '文件']] as const) {
    const tools = successful.filter(tool => tool.kind === kind)
    const targets = new Set(tools.map(targetIdentity).filter(Boolean))
    if (targets.size) fields.push(`已${verb} ${targets.size} 个${unit}`)
    const unnamed = tools.filter(tool => !targetIdentity(tool)).length
    if (unnamed) fields.push(`完成 ${unnamed} 次${verb}`)
  }
  const commands = successful.filter(tool => tool.kind === 'command').length
  if (commands) fields.push(`已执行 ${commands} 次命令`)
  const searches = successful.filter(tool => tool.kind === 'search' || tool.kind === 'web').length
  if (searches) fields.push(`完成 ${searches} 次检索`)
  const others = successful.filter(tool => tool.kind === 'other' || tool.kind === 'question').length
  if (others) fields.push(`完成 ${others} 次工具调用`)
  const failed = facts.tools.filter(tool => tool.failed).length
  if (failed) fields.push(`${failed} 次调用失败`)
  const unfinished = facts.tools.filter(tool => !tool.completed).length
  if (unfinished) fields.push(`${unfinished} 项${active && !facts.terminal ? '进行中' : '未完成'}`)
  return fields.join(' · ')
}

export interface RunActivityPresentation {
  runId: string | null
  label: string
  animation: FoxAnimation
  startedAt: number | null
  endedAt: number | null
  active: boolean
  urgent: boolean
  progress?: number
  outcome?: 'completed' | 'failed' | 'cancelled' | 'interrupted'
}

export function presentRunActivity(events: readonly RunEventRecord[], options: {
  runId?: string | null; state?: string | null; startedAt?: number | null; endedAt?: number | null
  approvalPending?: boolean; questionPending?: boolean
  currentPreview?: 'answer' | 'reasoning'
}): RunActivityPresentation {
  const runId = options.runId ?? null
  const idle: RunActivityPresentation = { runId, label: '准备就绪', animation: 'idle', startedAt: null, endedAt: null, active: false, urgent: true }
  if (!runId) return idle
  const facts = runFacts(events, runId)
  const state = options.state
  const startedAt = facts.events.find(item => item.eventType === 'run.started')?.createdAt ?? options.startedAt ?? null
  const endedAt = facts.terminal?.createdAt ?? options.endedAt ?? null
  const base = { ...idle, startedAt, endedAt, active: true, urgent: false }
  const result = (label: string, animation: FoxAnimation, urgent = false): RunActivityPresentation => ({ ...base, label, animation, urgent })
  // The persisted current-run state takes precedence over subordinate tool events.
  const terminal = state && terminalStates.has(state) ? state : facts.terminal?.eventType.slice(4)
  if (terminal) {
    const outcome = terminal === 'completed' || terminal === 'cancelled' || terminal === 'interrupted' ? terminal : 'failed'
    return { ...base, active: false, urgent: true, outcome,
      label: { completed: '已完成', cancelled: '已停止', interrupted: '已中断', failed: '处理失败' }[outcome],
      animation: outcome === 'completed' ? 'happy' : outcome === 'cancelled' ? 'idle' : 'alerting' }
  }
  if (state === 'cancelling') return result('正在停止', 'loading', true)
  if (options.approvalPending || state === 'waiting_approval') return result('等待你的确认', 'listening', true)
  if (options.questionPending) return result('等待你的回答', 'listening', true)
  if (state === 'retry_scheduled') return result('等待重试', 'humming', true)
  if (state === 'compacting') return result('正在整理上下文', 'loading')
  const tool = facts.tools.filter(tool => !tool.completed).sort((a, b) => b.seq - a.seq)[0]
  if (tool) {
    if (tool.progress !== undefined) return { ...result(`正在处理 · ${Math.round(tool.progress)}%`, 'progress'), progress: tool.progress }
    const labels: Record<ToolKind, [string, FoxAnimation]> = {
      read: ['正在读取文件', 'searching'], directory: ['正在查看目录', 'searching'],
      search: ['正在检索资料', 'searching'], web: ['正在搜索网页', 'radar'],
      command: ['正在执行命令', 'orbit'], write: ['正在修改文件', 'writing'],
      question: ['正在发起提问', 'orbit'], other: ['正在调用工具', 'orbit'],
    }
    return result(...labels[tool.kind])
  }
  if (options.currentPreview === 'answer') return result('正在书写回复', 'writing')
  if (options.currentPreview === 'reasoning') return result('正在思考', 'thinking')
  const waiting = activeModelWaiting(facts.events, { runId, runState: state })
  if (waiting) return result(waiting.phase === 'context_compaction' ? '正在整理上下文' : '等待回复', waiting.phase === 'context_compaction' ? 'loading' : 'receiving')
  const latest = [...facts.events].reverse().find(item => ['reasoning.delta', 'message.delta', 'message.started', 'message.completed', 'tool.started', 'tool.completed', 'tool.failed', 'run.phase', 'run.retrying', 'context.compaction.started', 'context.compaction.dispatched', 'context.compaction.completed', 'context.compaction.failed', 'user.question.requested', 'user.question.responded'].includes(item.eventType))
  if (latest?.eventType === 'run.retrying') return result('等待重试', 'humming')
  if (latest?.eventType === 'user.question.requested') return result('等待你的回答', 'listening', true)
  if (latest?.eventType === 'context.compaction.completed') return result('正在处理', 'loading')
  if (latest?.eventType.startsWith('context.compaction.')) return result(latest.eventType === 'context.compaction.failed' ? '上下文整理失败' : '正在整理上下文', latest.eventType === 'context.compaction.failed' ? 'alerting' : 'loading', latest.eventType === 'context.compaction.failed')
  if (latest?.eventType === 'run.phase') {
    if (latest.event.phase === 'preparing') return result('正在准备', 'loading')
    if (latest.event.phase === 'finalizing') return result('正在收尾', 'loading')
    if (latest.event.phase === 'request_sent') return result('等待回复', 'receiving')
  }
  if (latest?.eventType === 'reasoning.delta') return result('正在思考', 'thinking')
  if (latest?.eventType === 'message.delta') return result('正在书写回复', 'writing')
  if (state === 'queued' || state === 'created') return result('等待开始', 'humming')
  return result('正在处理', 'loading')
}

export function presentConversationActivity(detail: ConversationDetail | null, conversationId?: string | null) {
  const selected = detail?.conversation.id === conversationId ? detail : null
  const runId = selected?.lastRun?.id
  const snapshot = selected ? snapshotForRun(selected) : null
  const preview = selected?.messages.find(message => message.runId === runId && message.status === 'streaming'
    && message.kernelPreview && previewBelongsToSnapshot(message, snapshot))
  return presentRunActivity(selected?.runtimeEvents ?? [], {
    runId,
    state: conversationRunState(selected),
    startedAt: selected?.lastRun?.startedAt,
    endedAt: selected?.lastRun?.finishedAt,
    approvalPending: pendingRuntimeApprovals(selected).some(approval => approval.runId === runId),
    questionPending: Boolean(runId && pendingRuntimeQuestion(selected)?.runId === runId),
    currentPreview: preview?.content.trim() ? 'answer' : preview?.kernelPreview?.reasoning?.trim() ? 'reasoning' : undefined,
  })
}
