import type { ChildRunRecord, RunEventRecord, ToolCallRecord } from '@/features/conversations/model/types'

const ACTIVE_CHILD_RUN_STATUSES = new Set<ChildRunRecord['status']>([
  'queued',
  'running',
  'cancelling',
])

const WEB_TOOL_NAMES = new Set(['web_search', 'web_read', 'http_request'])

export interface ConversationUsage {
  inputTokens: number
  outputTokens: number
  cacheReadTokens: number
  cacheWriteTokens: number
  totalTokens: number
}

const USAGE_FIELDS = ['inputTokens', 'outputTokens', 'cacheReadTokens', 'cacheWriteTokens', 'totalTokens'] as const

function runtimeUsageCounters(event?: RunEventRecord): ConversationUsage {
  const value = event?.event
  const counter = (field: typeof USAGE_FIELDS[number]) => {
    const candidate = value?.[field]
    return typeof candidate === 'number' && Number.isFinite(candidate) && candidate >= 0 ? candidate : 0
  }
  const inputTokens = counter('inputTokens')
  const outputTokens = counter('outputTokens')
  const cacheReadTokens = counter('cacheReadTokens')
  const cacheWriteTokens = counter('cacheWriteTokens')
  return {
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    totalTokens: Math.max(counter('totalTokens'), inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens),
  }
}

export function latestConversationContextUsage(events: RunEventRecord[]): ConversationUsage {
  const usageEvents = events.filter((event) => event.eventType === 'usage.updated')
  const latest = usageEvents.at(-1)
  if (!latest) return runtimeUsageCounters()
  const previous = [...usageEvents].reverse().find((event) => event.runId === latest.runId && event.seq < latest.seq)
  const latestCounters = runtimeUsageCounters(latest)
  if (!previous) return latestCounters
  const previousCounters = runtimeUsageCounters(previous)
  const countersAreCumulative = USAGE_FIELDS.every((field) => latestCounters[field] >= previousCounters[field])
  if (!countersAreCumulative) return latestCounters
  const inputTokens = latestCounters.inputTokens - previousCounters.inputTokens
  const outputTokens = latestCounters.outputTokens - previousCounters.outputTokens
  const cacheReadTokens = latestCounters.cacheReadTokens - previousCounters.cacheReadTokens
  const cacheWriteTokens = latestCounters.cacheWriteTokens - previousCounters.cacheWriteTokens
  return {
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    totalTokens: Math.max(
      latestCounters.totalTokens - previousCounters.totalTokens,
      inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens,
    ),
  }
}

export function childRunIsActive(run: Pick<ChildRunRecord, 'status'>) {
  return ACTIVE_CHILD_RUN_STATUSES.has(run.status)
}

export function childRunStatusLabel(status: ChildRunRecord['status']) {
  switch (status) {
    case 'queued': return '已接收'
    case 'running': return '运行中'
    case 'cancelling': return '取消中'
    case 'completed': return '已完成'
    case 'failed': return '异常结束'
    case 'cancelled': return '已取消'
    case 'interrupted': return '已中断'
  }
}

export type ChildRunStatusTone = 'info' | 'success' | 'danger' | 'neutral'

export interface ChildRunStatusPresentation {
  label: string
  detail: string
  tone: ChildRunStatusTone
}

export function childRunStatusPresentation(status: ChildRunRecord['status']): ChildRunStatusPresentation {
  switch (status) {
    case 'queued': return { label: '已接收', detail: '任务已接收，正在等待可用的子 Agent', tone: 'info' }
    case 'running': return { label: '运行中', detail: '子 Agent 正在执行任务，状态会自动更新', tone: 'info' }
    case 'cancelling': return { label: '取消中', detail: '已发送取消信号，正在等待子 Agent 停止', tone: 'neutral' }
    case 'completed': return { label: '已完成', detail: '子 Agent 已返回运行结果', tone: 'success' }
    case 'failed': return { label: '异常结束', detail: '运行失败，子 Agent 已停止', tone: 'danger' }
    case 'cancelled': return { label: '已取消', detail: '任务已取消，子 Agent 已停止', tone: 'neutral' }
    case 'interrupted': return { label: '已中断', detail: '运行被中断，子 Agent 已停止', tone: 'danger' }
  }
}

export function formatRelativeUpdate(timestamp: number, now = Date.now()) {
  const elapsedMs = Math.max(0, now - timestamp)
  if (elapsedMs < 60_000) return '刚刚更新'
  const minutes = Math.floor(elapsedMs / 60_000)
  if (minutes < 60) return `${minutes} 分钟前更新`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时前更新`
  const days = Math.floor(hours / 24)
  return `${days} 天前更新`
}

export function childRunCounts(runs: ChildRunRecord[]) {
  return runs.reduce((counts, run) => {
    if (childRunIsActive(run)) counts.active += 1
    else if (run.status === 'completed') counts.completed += 1
    else counts.failed += 1
    return counts
  }, { active: 0, completed: 0, failed: 0 })
}

export function formatDurationMs(durationMs: number) {
  const safeDuration = Math.max(0, durationMs)
  if (safeDuration < 1_000) return '< 1 秒'
  const seconds = Math.floor(safeDuration / 1_000)
  if (seconds < 60) return `${seconds} 秒`
  const minutes = Math.floor(seconds / 60)
  const remainingSeconds = seconds % 60
  if (minutes < 60) return remainingSeconds ? `${minutes} 分 ${remainingSeconds} 秒` : `${minutes} 分钟`
  const hours = Math.floor(minutes / 60)
  const remainingMinutes = minutes % 60
  return remainingMinutes ? `${hours} 小时 ${remainingMinutes} 分` : `${hours} 小时`
}

export function childRunDuration(run: Pick<ChildRunRecord, 'createdAt' | 'startedAt' | 'finishedAt'>, now = Date.now()) {
  const startedAt = run.startedAt ?? run.createdAt
  return formatDurationMs((run.finishedAt ?? now) - startedAt)
}

export function normalizeBrowserUrl(value: string) {
  const trimmed = value.trim()
  if (!trimmed) return null
  const withProtocol = /^[a-z][a-z\d+.-]*:/i.test(trimmed) ? trimmed : `https://${trimmed}`
  try {
    const parsed = new URL(withProtocol)
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return null
    return parsed.toString()
  } catch {
    return null
  }
}

export function isWebToolCall(tool: Pick<ToolCallRecord, 'toolName'>) {
  return WEB_TOOL_NAMES.has(tool.toolName)
}

export interface WebActivitySummary {
  label: string
  target: string
  navigableUrl: string | null
}

export function webActivitySummary(tool: Pick<ToolCallRecord, 'toolName' | 'input'>): WebActivitySummary {
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
  const query = typeof input.query === 'string' ? input.query.trim() : ''
  const rawUrl = typeof input.url === 'string' ? input.url.trim() : ''
  const navigableUrl = rawUrl
    ? normalizeBrowserUrl(rawUrl)
    : query
      ? `https://duckduckgo.com/?q=${encodeURIComponent(query)}`
      : null
  return {
    label: tool.toolName === 'web_search' ? '网页搜索' : tool.toolName === 'web_read' ? '读取网页' : 'HTTP 请求',
    target: query || rawUrl || '未记录目标',
    navigableUrl,
  }
}
