/**
 * Presentation rules for the mid-run supplementary request strip
 * ("运行中补充要求").
 *
 * These are pure so the four-state history, the terminal-run case and the
 * read-only history case can be verified without a DOM. The Host owns every
 * durable state; this module only decides what the user sees.
 */

export interface SteeringRowView {
  messageId: string
  content: string
  status: string
}

export interface SteeringStripInput {
  /** Whether a Run is currently active for this conversation (UI state). */
  active: boolean
  /** The Run this listing describes, as resolved by the Host. */
  runId: string | null
  runState: string | null
  /** Whether the Host would still accept a new request for that Run. */
  acceptsSteering: boolean
  rows: SteeringRowView[]
}

export interface SteeringStripView {
  /** Nothing to show at all. */
  visible: boolean
  /** The editor may be offered. */
  editable: boolean
  /** Records are shown for auditing even though the Run is over. */
  historyOnly: boolean
  label: string
  hint: string
  outstanding: number
  pending: number
}

export const STEERING_STATUS_COPY: Record<string, { label: string; title: string }> = {
  received: { label: '已接收', title: '已持久化，将在下一个安全分发边界交给模型' },
  delivered: { label: '已投递', title: '已进入模型输入，等待模型响应提交' },
  applied: { label: '已应用', title: '模型已在响应中处理该要求' },
  cancelled: { label: '未应用', title: '运行在模型响应前结束，该要求没有交给模型' },
}

export function steeringStatusCopy(status: string) {
  return STEERING_STATUS_COPY[status] ?? { label: status, title: '' }
}

/**
 * Decide what the strip shows.
 *
 * The editor is offered only when the Host says the Run still accepts input.
 * When the Run is over, its records stay visible (so the user can check whether
 * each request was applied) but new text is not accepted: a finished task cannot
 * be steered, and the enqueue command would refuse it anyway.
 */
export function steeringStripView(input: SteeringStripInput): SteeringStripView {
  const pending = input.rows.filter(
    (row) => row.status === 'received' || row.status === 'delivered',
  ).length
  const outstanding = input.rows
    .filter((row) => row.status === 'received' || row.status === 'delivered')
    .reduce((total, row) => total + new TextEncoder().encode(row.content).length, 0)
  const editable = input.active && input.acceptsSteering
  const historyOnly = !editable && input.rows.length > 0
  return {
    visible: input.rows.length > 0,
    editable,
    historyOnly,
    label: historyOnly ? '补充要求记录（已结束）' : '运行中补充要求',
    hint: historyOnly
      ? `该任务已结束（${input.runState ?? '终态'}），以下记录保留用于核对；补充要求只能提交给正在运行的任务。`
      : '要求会持久保存，只在下一个安全分发边界交给模型；不会改变已冻结的任务，也不会新增工具授权。',
    outstanding,
    pending,
  }
}
