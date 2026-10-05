/**
 * Presentation rules for the mid-run supplementary request experience
 * ("运行中补充要求").
 *
 * These are pure so the lane split, the four durable states, the terminal-run
 * case and the read-only history case can be verified without a DOM. The Host
 * owns every durable state; this module only decides what the user sees and
 * which actions the composer may offer.
 *
 * Two lanes exist, and the user chooses between them at submit time:
 *
 * - `current` ("补充到当前任务") — bound to the active Run's next dispatch
 *   boundary, so it walks received → delivered → applied (cancelled terminal).
 * - `next-turn` ("排队，下一轮处理") — durably held for the conversation's next
 *   task. The active Run never carries it, so it stays queued while that Run is
 *   live and survives the Run ending; it is never silently dropped.
 *
 * The status copy states only what durable evidence proves. A message the Host
 * merely received is never described as one the model followed.
 */

export interface SteeringRowView {
  messageId: string
  content: string
  status: string
  /** `current` or `next-turn`; absent is treated as `current`. */
  lane?: string
}

export const STEERING_LANE_CURRENT = 'current'
export const STEERING_LANE_NEXT_TURN = 'next-turn'

export function steeringLaneOf(row: { lane?: string }) {
  return row.lane === STEERING_LANE_NEXT_TURN ? STEERING_LANE_NEXT_TURN : STEERING_LANE_CURRENT
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
  /** Something is worth showing above the composer. */
  visible: boolean
  /** The composer may offer "补充到当前任务" / "排队，下一轮处理". */
  editable: boolean
  /** Records are shown for auditing even though the Run is over. */
  historyOnly: boolean
  label: string
  hint: string
  /** Outstanding UTF-8 bytes queued on the `current` lane. */
  outstanding: number
  /** Rows queued on the `current` lane that no model response has answered. */
  pending: number
  /** Every row of the listing, in durable seq order. */
  rows: SteeringRowView[]
  /** Rows held for the next task that the user can still remove. */
  queued: SteeringRowView[]
  /** Rows of the `current` lane whose model response never committed. */
  unapplied: SteeringRowView[]
  /**
   * The Run is over but text queued for the next task remains. The surface must
   * offer an explicit way to carry it forward instead of dropping it.
   */
  retainedForNextTask: boolean
}

export const STEERING_STATUS_COPY: Record<string, { label: string; title: string }> = {
  received: {
    label: '已接收',
    title: '已持久保存，将在当前任务下一个安全分发边界交给模型（尚未交给模型）',
  },
  queued: {
    label: '等待生效',
    title: '已持久保存并留在队列中等待下一轮；当前任务不会处理它',
  },
  delivered: {
    label: '已交给模型',
    title: '已进入某个模型请求的输入，等待该轮响应提交结果',
  },
  applied: {
    label: '已应用',
    title: '模型响应已提交，该要求属于已响应请求的一部分',
  },
  cancelled: {
    label: '未应用',
    title: '任务在该要求被交给模型前结束，它没有进入任何模型请求',
  },
}

/** The durable state a row's label is drawn from, given its lane. */
export function steeringRowState(row: SteeringRowView) {
  if (row.status === 'received' && steeringLaneOf(row) === STEERING_LANE_NEXT_TURN) return 'queued'
  return row.status
}

export function steeringStatusCopy(status: string, lane?: string) {
  const state = status === 'received' && lane === STEERING_LANE_NEXT_TURN ? 'queued' : status
  return STEERING_STATUS_COPY[state] ?? { label: state, title: '' }
}

const encoder = new TextEncoder()

function utf8Bytes(value: string) {
  return encoder.encode(value).length
}

function queuedBytes(rows: SteeringRowView[]) {
  return rows.reduce((total, row) => total + utf8Bytes(row.content), 0)
}

/**
 * Decide what the supplementary-request surface shows.
 *
 * The composer's steering actions are offered only when the Host says the Run
 * still accepts input. When the Run is over, its records stay visible (so the
 * user can check whether each request was applied) and any text queued for the
 * next task is still listed, because that queue outlives the Run.
 */
export function steeringStripView(input: SteeringStripInput): SteeringStripView {
  const currentRows = input.rows.filter((row) => steeringLaneOf(row) === STEERING_LANE_CURRENT)
  const queued = input.rows.filter(
    (row) => steeringLaneOf(row) === STEERING_LANE_NEXT_TURN && row.status === 'received',
  )
  const unapplied = currentRows.filter(
    (row) => row.status === 'received' || row.status === 'delivered' || row.status === 'cancelled',
  )
  const editable = input.active && input.acceptsSteering
  const historyOnly = !editable && input.rows.length > 0
  const hasLiveQueue = input.active && queued.length > 0
  return {
    visible: input.rows.length > 0,
    editable,
    historyOnly,
    label: historyOnly ? '补充要求记录（已结束）' : '运行中补充要求',
    hint: historyOnly
      ? `该任务已结束（${input.runState ?? '终态'}），以下记录保留用于核对；${
          queued.length > 0
            ? '其中排队的补充要求仍保留，可作为新任务提交。'
            : '补充要求只能提交给正在运行的任务。'
        }`
      : hasLiveQueue
        ? '当前任务只会处理「补充到当前任务」的内容；排队的补充要求保留到下一轮，不改变当前任务，也不新增工具授权。'
        : '要求会持久保存，只在下一个安全分发边界交给模型；不会改变已冻结的任务，也不会新增工具授权。',
    outstanding: queuedBytes(currentRows.filter((row) => row.status === 'received' || row.status === 'delivered')),
    pending: currentRows.filter((row) => row.status === 'received' || row.status === 'delivered').length,
    rows: input.rows,
    queued,
    unapplied,
    retainedForNextTask: !input.active && queued.length > 0,
  }
}

/** What the main composer must offer while a Run is in flight. */
export interface SteeringComposerInput extends SteeringStripInput {}

export interface SteeringComposerView {
  /** Primary action: hand the text to the active Run at its next boundary. */
  canSubmitToCurrentTask: boolean
  /** Secondary action: hold the text for the conversation's next task. */
  canQueueForNextTurn: boolean
  primaryLabel: string
  secondaryLabel: string
  /** Text the user queued for the next task and can still take back. */
  queued: SteeringRowView[]
  /**
   * The Run is over but queued text remains. The surface must offer an explicit
   * way to carry it forward instead of dropping it.
   */
  retainedForNextTask: boolean
  retainedNotice: string | null
  /**
   * Stop is a separate control from every submit path: entering text must never
   * arm it, and no submit path may stop the Run.
   */
  stopIsSeparateControl: true
}

export function steeringComposerView(input: SteeringComposerInput): SteeringComposerView {
  const rows = input.rows ?? []
  const queued = rows.filter(
    (row) => steeringLaneOf(row) === STEERING_LANE_NEXT_TURN && row.status === 'received',
  )
  const accepts = input.active && input.acceptsSteering
  const retainedForNextTask = !input.active && queued.length > 0
  return {
    canSubmitToCurrentTask: accepts,
    canQueueForNextTurn: accepts,
    primaryLabel: '补充到当前任务',
    secondaryLabel: '排队，下一轮处理',
    queued,
    retainedForNextTask,
    retainedNotice: retainedForNextTask
      ? `有 ${queued.length} 条补充要求排队等待下一轮，本任务已结束（${input.runState ?? '终态'}）。它们不会被自动执行，也不会被丢弃。`
      : null,
    stopIsSeparateControl: true,
  }
}

export interface SteeringSubmitModeInput {
  /** The conversation is Host-backed (the desktop runtime is available). */
  runtimeControlled: boolean
  /** The composer's own send/stop state. */
  status: 'ready' | 'streaming'
  /** The Host would still accept a supplementary request for the active Run. */
  acceptsSteering: boolean
  /** An approval, question, goal confirmation, plan revision or workflow gate owns the composer. */
  decisionPending: boolean
  /** A runtime question is waiting for an answer. */
  hasActiveQuestion: boolean
}

/**
 * Whether the composer's primary action means "补充到当前任务".
 *
 * A pending decision always wins. While an approval is waiting, the composer is
 * the approval surface: supplementary text must not be able to stand in for the
 * decision the Run is parked on, and a Run parked on an approval is not
 * accepting steering input anyway. The same applies to a runtime question.
 */
export function steeringSubmitMode(input: SteeringSubmitModeInput): boolean {
  return input.runtimeControlled
    && input.status === 'streaming'
    && input.acceptsSteering
    && !input.decisionPending
    && !input.hasActiveQuestion
}
