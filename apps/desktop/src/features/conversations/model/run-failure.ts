import type { RunEventRecord, RunRecord } from './types'

/**
 * A run that ended in an abnormal terminal state. `kind` distinguishes a hard
 * failure from an interruption so the UI can word the two cases differently.
 */
export type RunFailureKind = 'failed' | 'interrupted'

export interface RunFailure {
  kind: RunFailureKind
  code: string
  message: string
}

export interface RunFailureInput {
  /** Persisted run rows loaded with the conversation; authoritative facts. */
  persistedRuns?: ReadonlyArray<Pick<RunRecord, 'id' | 'status' | 'errorCode' | 'errorMessage'>> | null
  /** Realtime / paginated runtime events; a finer-grained supplemental source. */
  events?: ReadonlyArray<Pick<RunEventRecord, 'runId' | 'eventType' | 'event'>> | null
}

export interface RunTerminalStates {
  /** One entry per failed/interrupted run id. */
  failures: Map<string, RunFailure>
  /** Runs the user cancelled on purpose; must not render as failures. */
  cancelled: Set<string>
}

const UNKNOWN_CODE = 'unknown'
const FAILED_MESSAGE = '运行失败'
const INTERRUPTED_MESSAGE = '运行中断'

function normalize(status: string | null | undefined): string {
  return (status ?? '').trim().toLowerCase()
}

function failureFor(
  kind: RunFailureKind,
  code?: string | null,
  message?: string | null,
): RunFailure {
  return {
    kind,
    code: code && code.length > 0 ? code : UNKNOWN_CODE,
    message: message && message.length > 0
      ? message
      : kind === 'failed' ? FAILED_MESSAGE : INTERRUPTED_MESSAGE,
  }
}

function kindForStatus(status: string | null | undefined): RunFailureKind | null {
  const normalized = normalize(status)
  if (normalized === 'failed') return 'failed'
  if (normalized === 'interrupted') return 'interrupted'
  return null
}

function kindForEvent(eventType: string): RunFailureKind | null {
  if (eventType === 'run.failed') return 'failed'
  if (eventType === 'run.interrupted') return 'interrupted'
  return null
}

/**
 * Derives the failure/cancellation map for a conversation from both the
 * persisted run rows and the loaded runtime events.
 *
 * Precedence: persisted terminal state is the fact source and always wins for a
 * given run id; events only fill runs whose persisted row is missing or still
 * non-terminal. This covers four cases with one code path — a run failing live,
 * reopening the conversation, restarting the app, and a run whose terminal event
 * fell outside the loaded event window.
 */
export function deriveRunTerminalStates(input: RunFailureInput): RunTerminalStates {
  const failures = new Map<string, RunFailure>()
  const cancelled = new Set<string>()

  // Realtime events first: they are the only source for a failure that is not
  // (yet) reflected by a persisted run snapshot.
  for (const event of input.events ?? []) {
    const kind = kindForEvent(event.eventType)
    if (!kind) {
      if (event.eventType === 'run.cancelled') {
        failures.delete(event.runId)
        cancelled.add(event.runId)
      }
      continue
    }
    if (failures.has(event.runId)) continue
    const payload = (event.event ?? {}) as { code?: string; message?: string }
    failures.set(event.runId, failureFor(kind, payload.code, payload.message))
  }

  // Persisted terminal runs override whatever the event stream suggested.
  for (const run of input.persistedRuns ?? []) {
    const kind = kindForStatus(run.status)
    if (kind) {
      cancelled.delete(run.id)
      failures.set(run.id, failureFor(kind, run.errorCode, run.errorMessage))
      continue
    }
    const status = normalize(run.status)
    if (status === 'cancelled') {
      failures.delete(run.id)
      cancelled.add(run.id)
      continue
    }
    // A completed run is authoritative even if a stale failure event lingered.
    if (status === 'completed') {
      failures.delete(run.id)
      cancelled.delete(run.id)
    }
  }

  return { failures, cancelled }
}

/** One failure per abnormal run id, keyed by run id. */
export function deriveRunFailures(input: RunFailureInput): Map<string, RunFailure> {
  return deriveRunTerminalStates(input).failures
}

/** Run ids the user cancelled on purpose, to render a neutral notice. */
export function deriveRunCancellations(input: RunFailureInput): Set<string> {
  return deriveRunTerminalStates(input).cancelled
}
