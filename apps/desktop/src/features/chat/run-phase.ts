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
import { formatRunElapsed } from './turn-process-timing'

export type RunPhase =
  | 'queued'
  | 'preparing'
  | 'awaiting_response'
  | 'streaming'
  | 'finalizing'
  | 'settled'

export const RUN_PHASE_LABELS: Record<RunPhase, string> = {
  queued: '排队等待执行',
  preparing: '正在准备上下文',
  awaiting_response: '等待模型响应',
  streaming: '正在生成',
  finalizing: '正在收尾',
  settled: '已完成',
}

const TERMINAL_EVENT_TYPES = new Set(['run.completed', 'run.cancelled', 'run.failed', 'run.interrupted'])

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
  options: { queuedAt?: number | null; now: number },
): RunPhaseTiming {
  const started = earliestEvent(events, 'run.started')
  const terminal = [...events]
    .reverse()
    .find((item) => TERMINAL_EVENT_TYPES.has(item.eventType) && Number.isFinite(item.createdAt))
  const queuedAt = typeof options.queuedAt === 'number' && Number.isFinite(options.queuedAt)
    ? options.queuedAt
    : null
  const endedAt = terminal?.createdAt ?? null

  if (!started) {
    // No execution start yet: this Run is still holding a place in the queue.
    return {
      phase: terminal ? 'settled' : 'queued',
      startedAt: null,
      endedAt,
      queueMs: queuedAt === null ? null : Math.max(0, (endedAt ?? options.now) - queuedAt),
      executionMs: null,
    }
  }

  const startedAt = started.createdAt
  const queueMs = queuedAt === null ? null : Math.max(0, startedAt - queuedAt)
  const executionMs = Math.max(0, (endedAt ?? options.now) - startedAt)
  if (terminal) return { phase: 'settled', startedAt, endedAt, queueMs, executionMs }

  const explicitPhase = latestPhase(events)
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
 * Header text for a live Run. Queue wait and execution time are reported
 * separately, because they answer different questions: how long the request waited
 * for its turn, and how long the model has been working.
 */
export function runPhaseTitle(timing: RunPhaseTiming): string {
  const label = RUN_PHASE_LABELS[timing.phase]
  if (timing.phase === 'queued') {
    return timing.queueMs === null ? label : `${label} · 已排队 ${formatRunElapsed(timing.queueMs)}`
  }
  if (timing.phase === 'awaiting_response' || timing.phase === 'preparing') {
    return timing.executionMs === null ? label : `${label} · 已等待 ${formatRunElapsed(timing.executionMs)}`
  }
  if (timing.phase === 'streaming' || timing.phase === 'finalizing') {
    return timing.executionMs === null ? label : `${label} · 已用时 ${formatRunElapsed(timing.executionMs)}`
  }
  return label
}
