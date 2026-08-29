import type { ChildRunRecord, ToolCallRecord } from '@/features/conversations/model/types'

const ACTIVE_CHILD_RUN_STATUSES = new Set<ChildRunRecord['status']>([
  'queued',
  'running',
  'cancelling',
])

const WEB_TOOL_NAMES = new Set(['web_search', 'web_read', 'http_request'])

export function childRunIsActive(run: Pick<ChildRunRecord, 'status'>) {
  return ACTIVE_CHILD_RUN_STATUSES.has(run.status)
}

export function childRunStatusLabel(status: ChildRunRecord['status']) {
  switch (status) {
    case 'queued': return '排队中'
    case 'running': return '执行中'
    case 'cancelling': return '取消中'
    case 'completed': return '已完成'
    case 'failed': return '失败'
    case 'cancelled': return '已取消'
    case 'interrupted': return '已中断'
  }
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
