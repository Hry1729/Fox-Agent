// Presentation model for background compute jobs (CONTRACTS §4 JobSnapshot).
// Pure helpers, unit-tested without a DOM; the card component binds them.

export type JobState =
  | 'queued'
  | 'running'
  | 'paused'
  | 'cancelled'
  | 'failed'
  | 'completed'

export interface JobSnapshot {
  jobId: string
  runId: string
  kind: string
  state: JobState
  cursor?: string | null
  progressDone: number
  progressTotal?: number | null
  resultRef?: string | null
  resultBytes?: number | null
  resultSha256?: string | null
  errorCode?: string | null
  errorMessage?: string | null
  startedAt: number
  updatedAt: number
  attempts: number
  resumable?: boolean
  cancelRequestedAt?: number | null
  cancelAcknowledgedAt?: number | null
}

export function isTerminal(state: JobState): boolean {
  return state === 'completed' || state === 'failed' || state === 'cancelled'
}

export function isCancellable(state: JobState): boolean {
  return state === 'queued' || state === 'running' || state === 'paused'
}

/** 0..1 when the total is known, otherwise null (indeterminate display). */
export function progressRatio(snapshot: JobSnapshot): number | null {
  const total = snapshot.progressTotal
  if (!total || total <= 0) return null
  return Math.min(1, Math.max(0, snapshot.progressDone / total))
}

export function stateLabel(state: JobState): string {
  switch (state) {
    case 'queued':
      return '排队中'
    case 'running':
      return '计算中'
    case 'paused':
      return '已暂停（可续做）'
    case 'cancelled':
      return '已取消'
    case 'failed':
      return '失败'
    case 'completed':
      return '已完成'
  }
}

export function kindLabel(kind: string): string {
  switch (kind) {
    case 'attachment_compute':
      return '附件计算'
    case 'office_batch':
      return 'Office 批量写入'
    default:
      return kind
  }
}

/** One-line progress text; never implies bytes the job did not report. */
export function progressText(snapshot: JobSnapshot): string {
  const done = snapshot.progressDone
  const total = snapshot.progressTotal
  if (snapshot.state === 'completed') return '计算完成'
  if (total && total > 0) {
    const pct = Math.floor((done / total) * 100)
    return `已处理 ${formatCount(done)} / ${formatCount(total)} 行（${pct}%）`
  }
  if (done > 0) return `已处理 ${formatCount(done)} 行`
  return stateLabel(snapshot.state)
}

export function canViewResult(snapshot: JobSnapshot): boolean {
  return snapshot.state === 'completed' && typeof snapshot.resultRef === 'string' && snapshot.resultRef.length > 0
}

function formatCount(value: number): string {
  return value.toLocaleString('zh-CN')
}
