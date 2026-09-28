import type { ConversationMessage, RunEventRecord } from '@/features/conversations/model/types'
import { answerDeltaFingerprint } from '@/features/conversations/model/runtime-delta-fingerprint'

export type RuntimeProcessGroup = {
  kind: 'process'
  key: string
  events: RunEventRecord[]
}

export type RuntimeResponseGroup = {
  kind: 'response'
  key: string
  text: string
}

export type RuntimeNoticeGroup = {
  kind: 'notice'
  key: string
  label: string
}

export type RuntimeDisplayGroup = RuntimeProcessGroup | RuntimeResponseGroup | RuntimeNoticeGroup

export type ProcessActivityKind =
  | 'localKnowledge' | 'remoteKnowledge' | 'knowledge' | 'read' | 'search' | 'write'
  | 'command' | 'web' | 'office' | 'question' | 'other'

export type ProcessActivityCount = { kind: ProcessActivityKind; count: number }

const boundaryLabels: Record<string, string> = {
  'run.retrying': '正在重试',
  'run.retry.completed': '已恢复并继续',
  'user.question.requested': '等待你的回答',
  'user.question.responded': '已收到回答',
  'run.failed': '运行失败',
  'run.interrupted': '运行中断',
  'run.cancelled': '运行已取消',
}

const processEvents = new Set(['reasoning.delta', 'tool.started', 'tool.updated', 'tool.completed'])

/** Kernel messages are replaceable round snapshots, never append-only deltas.
 * Only explicit Host round ownership joins tools to messages. No timestamp or
 * response-length guesses, and no change to the execution snapshot/history.
 */
export function projectKernelGroups(runId: string, messages: readonly ConversationMessage[], events: readonly RunEventRecord[]): RuntimeDisplayGroup[] | null {
  type Round = { message?: ConversationMessage; reasoning: RunEventRecord[]; tools: RunEventRecord[] }
  const rounds = new Map<string, Round>()
  const roundFor = (checkpoint: string) => {
    if (!/^[1-9]\d{0,18}$/.test(checkpoint)) return null
    if (!rounds.has(checkpoint)) rounds.set(checkpoint, { reasoning: [], tools: [] })
    return rounds.get(checkpoint)!
  }
  for (const message of messages) {
    const prefix = `kernel-message:${runId}:`
    if (message.runId !== runId || !message.id.startsWith(prefix)) return null
    const round = roundFor(message.id.slice(prefix.length))
    if (!round || round.message) return null
    round.message = message
  }
  const notices: RuntimeNoticeGroup[] = []
  const seen = new Set<number>()
  for (const event of [...events].filter(item => item.runId === runId).sort((a, b) => a.seq - b.seq)) {
    if (seen.has(event.seq)) continue
    seen.add(event.seq)
    if (processEvents.has(event.eventType)) {
      const source = event.event.source
      const checkpoint = event.eventType === 'reasoning.delta' && typeof source === 'string' && source.startsWith('kernel-model:')
        ? source.slice('kernel-model:'.length) : event.event.kernelCheckpointSeq
      const round = typeof checkpoint === 'string' ? roundFor(checkpoint) : null
      if (!round) return null // Old/partial history remains readable through the existing fallback.
      ;(event.eventType === 'reasoning.delta' ? round.reasoning : round.tools).push(event)
    } else if (Object.hasOwn(boundaryLabels, event.eventType)) {
      notices.push({ kind: 'notice', key: `${runId}:notice:${event.seq}`, label: boundaryLabels[event.eventType] })
    }
  }
  const groups: RuntimeDisplayGroup[] = []
  const appendProcess = (key: string, items: RunEventRecord[]) => {
    if (!items.length) return
    const previous = groups.at(-1)
    if (previous?.kind === 'process') previous.events.push(...items)
    else groups.push({ kind: 'process', key, events: [...items] })
  }
  for (const [checkpoint, round] of [...rounds].sort(([a], [b]) => BigInt(a) < BigInt(b) ? -1 : 1)) {
    const preview = round.message?.kernelPreview
    // A preview is a full replacement (including empty reasoning), not a delta.
    const reasoning = preview?.reasoning !== undefined ? [{
      runId, seq: round.reasoning[0]?.seq ?? 0, eventType: 'reasoning.delta',
      event: { type: 'reasoning.delta', delta: preview.reasoning, source: `kernel-model:${checkpoint}` },
      createdAt: round.message!.updatedAt,
    }] : round.reasoning
    appendProcess(`${runId}:kernel:${checkpoint}:reasoning`, reasoning)
    if (round.message?.content.trim()) groups.push({ kind: 'response', key: `${runId}:kernel:${checkpoint}:response`, text: round.message.content })
    appendProcess(`${runId}:kernel:${checkpoint}:tools`, round.tools)
  }
  return groups.length ? [...groups, ...notices] : null
}

/** A bounded read projection: the answer lives once in messages.content. */
export function answerDeltaLength(event: RunEventRecord): number | null {
  if (event.eventType !== 'message.delta') return null
  const length = event.event.deltaLength
  return typeof length === 'number' && Number.isInteger(length) && length >= 0 ? length : null
}

/** No inferred order for old or incomplete histories. All answer text must be covered. */
export function projectRuntimeGroups(runId: string, content: string, events: readonly RunEventRecord[]): RuntimeDisplayGroup[] | null {
  const ordered = [...events].filter(item => item.runId === runId).sort((a, b) => a.seq - b.seq)
  const seen = new Set<number>()
  const groups: RuntimeDisplayGroup[] = []
  const tools = new Map<string, RuntimeProcessGroup>()
  let offset = 0
  let answerEvents = 0
  let current: RuntimeDisplayGroup | undefined
  const flush = () => { current = undefined }
  for (const event of ordered) {
    if (seen.has(event.seq)) continue
    seen.add(event.seq)
    const length = answerDeltaLength(event)
    if (event.eventType === 'message.delta') {
      if (length === null || offset + length > content.length) return null
      answerEvents++
      const text = content.slice(offset, offset + length)
      if (event.event.deltaFingerprint !== answerDeltaFingerprint(text)) return null
      offset += length
      if (!text) continue
      if (current?.kind === 'response') current.text += text
      else {
        current = { kind: 'response', key: `${runId}:response:${event.seq}`, text }
        groups.push(current)
      }
      continue
    }
    if (Object.hasOwn(boundaryLabels, event.eventType)) {
      flush()
      groups.push({ kind: 'notice', key: `${runId}:notice:${event.seq}`, label: boundaryLabels[event.eventType] })
      continue
    }
    if (!processEvents.has(event.eventType)) continue
    if (event.eventType.startsWith('tool.')) {
      const callId = typeof event.event.toolCallId === 'string' && event.event.toolCallId
        ? event.event.toolCallId : `tool-${event.seq}`
      const owner = tools.get(callId)
      if (owner) {
        owner.events.push(event)
        continue
      }
    }
    if (current?.kind !== 'process') {
      current = { kind: 'process', key: `${runId}:process:${event.seq}`, events: [] }
      groups.push(current)
    }
    current.events.push(event)
    if (event.eventType.startsWith('tool.')) {
      const callId = typeof event.event.toolCallId === 'string' && event.event.toolCallId
        ? event.event.toolCallId : `tool-${event.seq}`
      tools.set(callId, current)
    }
  }
  if (offset !== content.length || (!answerEvents && (!groups.length || Boolean(content)))) return null
  return groups
}

export function processActivityKind(name: string, input?: unknown): ProcessActivityKind {
  const value = name.toLowerCase()
  if (value === 'query_kb') return 'remoteKnowledge'
  if (/^(search_knowledge|read_knowledge_document|list_knowledge_bases)$/.test(value)) {
    const data = input && typeof input === 'object' && !Array.isArray(input) ? input as Record<string, unknown> : {}
    const many = data.targets ?? data.references
    const targets = Array.isArray(many) ? many : [data.target ?? data.reference]
    const sources = targets.map(item => item?.source)
    if (sources.length && sources.every(source => source === 'local')) return 'localKnowledge'
    if (sources.length && sources.every(source => source === 'remote') || typeof data.knowledgeBaseId === 'string') return 'remoteKnowledge'
    return 'knowledge'
  }
  if (/^(read|view|open|read_attachment)/.test(value)) return 'read'
  if (/^(grep|find|glob|search)/.test(value)) return 'search'
  if (/^(write|edit|apply_patch|str_replace|create|patch)/.test(value)) return 'write'
  if (/^(run_command|exec_command|bash|pwsh|write_stdin|terminal_)/.test(value)) return 'command'
  if (/^(web_|http_|fetch|browser)/.test(value)) return 'web'
  if (value.startsWith('office_')) return 'office'
  if (value === 'call_mcp_tool') {
    const data = input && typeof input === 'object' && !Array.isArray(input) ? input as Record<string, unknown> : {}
    const server = typeof data.server === 'string' ? data.server : typeof data.serverName === 'string' ? data.serverName : ''
    return /(?:^|[-_])(?:fox[-_]?)?office(?:$|[-_])/i.test(server) ? 'office' : 'other'
  }
  if (/^(ask_user_question|request_user_input)/.test(value)) return 'question'
  return 'other'
}

export function summarizeProcessActivity(events: readonly RunEventRecord[]): ProcessActivityCount[] {
  const counts = new Map<ProcessActivityKind, number>()
  const calls = new Set<string>()
  for (const event of [...events].sort((a, b) => a.seq - b.seq)) {
    if (!event.eventType.startsWith('tool.')) continue
    const callId = typeof event.event.toolCallId === 'string' && event.event.toolCallId
      ? event.event.toolCallId : `tool-${event.seq}`
    if (calls.has(callId)) continue
    calls.add(callId)
    const kind = processActivityKind(typeof event.event.tool === 'string' ? event.event.tool : '', event.event.input)
    counts.set(kind, (counts.get(kind) ?? 0) + 1)
  }
  return [...counts].map(([kind, count]) => ({ kind, count })).sort((a, b) => b.count - a.count)
}

export function processGroupTitle(events: readonly RunEventRecord[]) {
  const counts = summarizeProcessActivity(events)
  if (!counts.length) return '已完成分析'
  const labels: Record<ProcessActivityKind, string> = {
    localKnowledge: '检索了本地知识库', remoteKnowledge: '检索了远程知识库', knowledge: '检索了知识库',
    read: '读取了文件', search: '已搜索代码', write: '修改了文件', command: '执行了命令',
    web: '访问了网页', office: '调用了办公工具', question: '询问了用户', other: '调用了工具',
  }
  const selected = counts.slice(0, 3).map(item => labels[item.kind])
  const title = selected.length < 2 ? selected[0] : `${selected.slice(0, -1).join('、')}并${selected.at(-1)}`
  return `${title}${counts.length > 3 ? '等' : ''}`
}

export function activeProcessGroupTitle(name?: string, input?: unknown) {
  if (!name) return '正在分析请求'
  const labels: Record<ProcessActivityKind, string> = {
    localKnowledge: '正在检索本地知识库', remoteKnowledge: '正在检索远程知识库',
    knowledge: '正在检索知识库', read: '正在读取文件', search: '正在搜索代码',
    write: '正在修改文件', command: '正在执行命令', web: '正在访问网页',
    office: '正在调用办公工具', question: '等待你的回答', other: '正在调用工具',
  }
  return labels[processActivityKind(name, input)]
}
