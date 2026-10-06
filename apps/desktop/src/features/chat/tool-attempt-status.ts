/** A tool result describes one attempt. Only a Run terminal state describes the task. */
export interface ToolAttempt {
  name: string
  seq: number
  lastSeq: number
  isError: boolean
  completed: boolean
  output?: unknown
}

export type ToolAttemptState = 'pending' | 'continued' | 'recovered' | 'denied' | 'cancelled' | 'blocked'
export type ToolAttemptReason = 'permission' | 'cancelled' | 'arguments' | 'environment' | 'other'

function resultCode(output: unknown): string {
  if (!output || typeof output !== 'object' || Array.isArray(output)) return ''
  const value = output as Record<string, unknown>
  const nested = value.error && typeof value.error === 'object' && !Array.isArray(value.error)
    ? value.error as Record<string, unknown> : null
  return [value.code, value.errorCode, value.status, nested?.code, nested?.errorCode]
    .filter((part): part is string => typeof part === 'string').join(' ').toLowerCase()
}

export function toolAttemptReason(output: unknown): ToolAttemptReason {
  const code = resultCode(output)
  if (/cancel|abort|user_stop/.test(code)) return 'cancelled'
  if (/permission|approval|denied|forbidden|unauthori[sz]ed/.test(code)) return 'permission'
  if (/invalid|argument|parameter|schema|parse/.test(code)) return 'arguments'
  if (/timeout|unavailable|connection|network|environment|not_found|spawn|io_error/.test(code)) return 'environment'
  return 'other'
}

export function toolAttemptState(
  attempt: ToolAttempt,
  attempts: readonly ToolAttempt[],
  latestEventSeq: number,
  runState: 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted',
): ToolAttemptState {
  if (attempts.some((later) => later.seq > attempt.lastSeq && later.name === attempt.name && later.completed && !later.isError)) return 'recovered'
  if (toolAttemptReason(attempt.output) === 'cancelled' || runState === 'cancelled') return 'cancelled'
  if (toolAttemptReason(attempt.output) === 'permission') return 'denied'
  if (runState === 'failed' || runState === 'interrupted') return 'blocked'
  if (runState === 'completed' || latestEventSeq > attempt.lastSeq) return 'continued'
  return 'pending'
}

export const TOOL_ATTEMPT_STATE_LABELS: Record<ToolAttemptState, string> = {
  pending: '等待调整',
  continued: '已继续处理',
  recovered: '后续同类调用成功',
  denied: '权限请求已拒绝',
  cancelled: '已取消',
  blocked: '运行未能继续',
}

export const TOOL_ATTEMPT_REASON_LABELS: Record<ToolAttemptReason, string> = {
  permission: '权限',
  cancelled: '取消',
  arguments: '参数',
  environment: '环境',
  other: '工具',
}
