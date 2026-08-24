import type { OperationEvent, OperationData, OperationStatus } from './model'

export interface OperationState {
  operationId: string
  lastSequence: number
  status: OperationStatus | 'idle'
  stage: OperationEvent['stage'] | null
  progress: number
  data: OperationData
  outcome: OperationData['outcome'] | null
  error: OperationEvent['error'] | null
  lastEvent: OperationEvent | null
}

export function createOperationState(operationId: string): OperationState {
  return {
    operationId,
    lastSequence: 0,
    status: 'idle',
    stage: null,
    progress: 0,
    data: {},
    outcome: null,
    error: null,
    lastEvent: null,
  }
}

export function reduceOperationEvent(state: OperationState, event: OperationEvent): OperationState {
  if (event.operationId !== state.operationId || event.sequence <= state.lastSequence) return state

  const data = event.data ? { ...state.data, ...event.data } : state.data
  const outcome = event.data?.outcome ?? state.outcome

  return {
    ...state,
    lastSequence: event.sequence,
    status: event.status,
    stage: event.stage ?? state.stage,
    progress: event.progress ?? state.progress,
    data,
    outcome,
    error: event.error ?? (event.status === 'running' || event.status === 'paused' ? null : state.error),
    lastEvent: event,
  }
}

export function reduceOperationEvents(state: OperationState, events: OperationEvent[]): OperationState {
  return events.reduce(reduceOperationEvent, state)
}

export function isOperationTerminal(status: OperationState['status']): boolean {
  return status === 'completed' || status === 'failed' || status === 'cancelled'
}

export function isPartialOperation(state: OperationState): boolean {
  return state.status === 'completed' && state.outcome === 'partial'
}

export function operationStatusLabel(state: Pick<OperationState, 'status' | 'outcome'>): string {
  if (state.status === 'completed' && state.outcome === 'partial') return '部分完成'
  const labels: Record<OperationState['status'], string> = {
    idle: '未开始',
    queued: '排队中',
    running: '处理中',
    paused: '已暂停',
    completed: '已完成',
    failed: '失败',
    cancelled: '已取消',
  }
  return labels[state.status]
}
