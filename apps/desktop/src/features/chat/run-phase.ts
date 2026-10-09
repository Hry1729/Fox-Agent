/**
 * Which stage of a Run the user is actually waiting in.
 *
 * Before this, every wait looked the same ("正在分析请求"), so a Run stuck in the
 * shared execution queue was indistinguishable from a Run whose model request was
 * already streaming. The stages below are derived only from durable events, and a
 * stage that cannot be observed for a given Run reports `null` rather than a guess.
 *
 * Boundaries a Run actually goes through, and who can observe them:
 *
 * | stage                     | observed here?                                  |
 * |---------------------------|------------------------------------------------|
 * | queued (execution slot)   | yes — the run has no `run.started` yet          |
 * | execution started         | yes — `run.started`                             |
 * | preparing context         | yes — `run.phase` = preparing                   |
 * | request handed to worker  | yes — `run.phase` = request_sent                |
 * | provider connection reuse | no — inside the engine worker, reported absent  |
 * | response headers          | no — inside the engine worker, reported absent  |
 * | first streamed chunk      | yes — first content event for the run           |
 * | finalized                 | yes — `run.phase` = finalizing / terminal event  |
 */

import type { RunEventRecord } from '@/features/conversations/model/types'
import { TERMINAL_RUN_EVENT_TYPES, formatRunElapsed } from './turn-process-timing'
import { activeModelWaiting, modelWaitingTitle, type ModelWaitingFact } from './run-status-facts'

export type RunPhase =
  | 'queued'
  | 'preparing'
  | 'awaiting_response'
  | 'awaiting_compaction'
  | 'preparing_compaction'
  | 'retry_scheduled'
  | 'compaction_failed'
  | 'streaming'
  | 'finalizing'
  | 'settled'

export const RUN_PHASE_LABELS: Record<RunPhase, string> = {
  queued: '准备执行',
  preparing: '正在准备上下文',
  awaiting_response: '等待模型响应',
  awaiting_compaction: '等待压缩响应',
  preparing_compaction: '正在准备压缩',
  retry_scheduled: '已安排重试',
  compaction_failed: '压缩未完成',
  streaming: '正在生成',
  finalizing: '正在收尾',
  settled: '已完成',
}

const TERMINAL_EVENT_TYPES = TERMINAL_RUN_EVENT_TYPES

/** Durable proof that the model has produced something the user can read. */
const CONTENT_EVENT_TYPES = new Set([
  'reasoning.delta',
  'message.started',
  'message.delta',
  'tool.started',
])

export interface RunPhaseTiming {
  phase: RunPhase
  /** When this Run left the shared execution queue (`run.started`). */
  startedAt: number | null
  endedAt: number | null
  /** Queue wait: enqueue → execution started. `null` when the enqueue time is unknown. */
  queueMs: number | null
  /** Execution time: execution started → now or terminal. `null` before execution starts. */
  executionMs: number | null
  modelWaiting?: ModelWaitingFact | null
}

function earliestEvent(events: readonly RunEventRecord[], eventType: string) {
  let found: RunEventRecord | undefined
  for (const item of events) {
    if (item.eventType !== eventType || !Number.isFinite(item.createdAt)) continue
    if (!found || item.seq < found.seq) found = item
  }
  return found
}

/** Durable phases this projection understands. Anything else is not a stage here. */
const KNOWN_PHASES = new Set(['preparing', 'request_sent', 'streaming', 'finalizing'])

function latestPhase(events: readonly RunEventRecord[]) {
  for (let index = events.length - 1; index >= 0; index--) {
    const item = events[index]
    if (CONTENT_EVENT_TYPES.has(item.eventType)) return 'streaming'
    if (item.eventType === 'context.compaction.failed') return 'compaction_failed'
    if (item.eventType === 'context.compaction.completed') return 'preparing'
    if (item.eventType === 'context.compaction.started') return 'preparing_compaction'
    if (item.eventType === 'context.compaction.dispatched') return 'context_compaction'
    if (item.eventType === 'run.retrying') return 'retry_scheduled'
    if (item.eventType !== 'run.phase') continue
    const phase = item.event?.phase
    // An unrecognised phase (a future or unrelated one) neither becomes a stage of
    // its own nor erases a stage this projection already knows about.
    if (typeof phase === 'string' && KNOWN_PHASES.has(phase)) return phase
  }
  return null
}

/**
 * Stage of one Run, from its own durable events only. `queuedAt` is the enqueue
 * time (the run and its user message are written in the same transaction), so the
 * queue wait is real rather than an estimate; without it the wait stays `null`.
 */
export function runPhaseTiming(
  events: readonly RunEventRecord[],
  options: { queuedAt?: number | null; now: number; runState?: string | null },
): RunPhaseTiming {
  events = [...events].sort((a, b) => a.seq - b.seq)
  const started = earliestEvent(events, 'run.started')
  const terminal = [...events]
    .reverse()
    .find((item) => TERMINAL_EVENT_TYPES.has(item.eventType) && Number.isFinite(item.createdAt))
  const queuedAt = typeof options.queuedAt === 'number' && Number.isFinite(options.queuedAt)
    ? options.queuedAt
    : null
  const endedAt = terminal?.createdAt ?? null
  const settled = Boolean(terminal || options.runState && ['completed', 'cancelled', 'failed', 'interrupted', 'budget_exhausted', 'approval_expired'].includes(options.runState))

  if (!started) {
    // No execution start yet: this Run is still holding a place in the queue.
    return {
      phase: settled ? 'settled' : 'queued',
      startedAt: null,
      endedAt,
      queueMs: queuedAt === null ? null : Math.max(0, (endedAt ?? options.now) - queuedAt),
      executionMs: null,
    }
  }

  const startedAt = started.createdAt
  const queueMs = queuedAt === null ? null : Math.max(0, startedAt - queuedAt)
  const executionMs = Math.max(0, (endedAt ?? options.now) - startedAt)
  if (settled) return { phase: 'settled', startedAt, endedAt, queueMs, executionMs }

  const modelWaiting = activeModelWaiting(events, { runState: options.runState })
  if (modelWaiting) return {
    phase: modelWaiting.phase === 'context_compaction' ? 'awaiting_compaction' : 'awaiting_response',
    startedAt, endedAt, queueMs, executionMs, modelWaiting,
  }

  const explicitPhase = latestPhase(events)
  if (options.runState === 'retry_scheduled') return { phase: 'retry_scheduled', startedAt, endedAt, queueMs, executionMs }
  if (explicitPhase === 'compaction_failed') return { phase: 'compaction_failed', startedAt, endedAt, queueMs, executionMs }
  if (explicitPhase === 'preparing_compaction') return { phase: 'preparing_compaction', startedAt, endedAt, queueMs, executionMs }
  if (explicitPhase === 'context_compaction') return { phase: 'awaiting_compaction', startedAt, endedAt, queueMs, executionMs }
  if (explicitPhase === 'retry_scheduled') return { phase: 'retry_scheduled', startedAt, endedAt, queueMs, executionMs }
  if (explicitPhase === 'finalizing') return { phase: 'finalizing', startedAt, endedAt, queueMs, executionMs }
  if (explicitPhase === 'preparing') return { phase: 'preparing', startedAt, endedAt, queueMs, executionMs }
  if (explicitPhase === 'request_sent') return { phase: 'awaiting_response', startedAt, endedAt, queueMs, executionMs }
  if (explicitPhase === 'streaming') return { phase: 'streaming', startedAt, endedAt, queueMs, executionMs }

  // No phase event (older Runs): a content event after `run.started` is the only
  // durable proof that generation began.
  const generating = events.some((item) => item.seq > started.seq && CONTENT_EVENT_TYPES.has(item.eventType))
  return { phase: generating ? 'streaming' : 'awaiting_response', startedAt, endedAt, queueMs, executionMs }
}

/**
 * Header text for a live Run.
 *
 * The label is the phase itself; the elapsed time is *not* glued onto it here,
 * because the header already renders the run's single timer next to the avatar.
 * Two timers in one row (one inside the label, one beside the avatar) reported the
 * same interval twice and made one state look like two.
 */
export function runPhaseTitle(timing: RunPhaseTiming): string {
  if (timing.modelWaiting) return modelWaitingTitle(timing.modelWaiting)
  return RUN_PHASE_LABELS[timing.phase]
}
