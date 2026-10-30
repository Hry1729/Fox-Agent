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
  /**
   * Backend stderr tail persisted with an external backend's `run.failed`
   * (`diagnostic.stderrTail`; redacted by the Host before it was written, at
   * most 4 KiB). Present only when the terminal event carried one.
   */
  stderrTail?: string
  /** Which failure the tail belongs to (`prompt_failed`, `backend_exited`, ...). */
  stderrTailSource?: string
  /**
   * The backend's JSON-RPC error `data` (`source: "rpc_error_data"`), redacted
   * by the Host and at most 4 KiB. Never part of `message`.
   */
  rpcErrorData?: string
}

/** One entry of `run.failed.diagnostics` (or the legacy single `diagnostic`). */
interface RunFailureDiagnostic {
  kind?: string
  source?: string
  /** `kind: "stderr_tail"`. */
  stderrTail?: string
  /** `kind: "rpc_error_data"`. */
  text?: string
}

export const RPC_ERROR_DATA_SOURCE = 'rpc_error_data'

interface ParsedDiagnostic { stderrTail?: string; source?: string; rpcErrorData?: string }

/**
 * Reads the Host's diagnostics. Newer Hosts write `diagnostics` (array: the
 * stderr tail, then the JSON-RPC `data`) plus `diagnostic` (the first entry);
 * older ones only `diagnostic` (always a stderr tail). Both are accepted.
 */
function diagnosticOf(payload: unknown): ParsedDiagnostic | null {
  const record = payload as { diagnostic?: RunFailureDiagnostic; diagnostics?: RunFailureDiagnostic[] } | null
  const entries: RunFailureDiagnostic[] = Array.isArray(record?.diagnostics)
    ? record!.diagnostics.filter((entry) => entry && typeof entry === 'object')
    : record?.diagnostic && typeof record.diagnostic === 'object' ? [record.diagnostic] : []
  const parsed: ParsedDiagnostic = {}
  for (const entry of entries) {
    const isData = entry.source === RPC_ERROR_DATA_SOURCE || entry.kind === RPC_ERROR_DATA_SOURCE
    if (isData) {
      const text = typeof entry.text === 'string' ? entry.text : ''
      if (text.trim() && parsed.rpcErrorData === undefined) parsed.rpcErrorData = text
      continue
    }
    const tail = typeof entry.stderrTail === 'string' ? entry.stderrTail : ''
    if (tail.trim() && parsed.stderrTail === undefined) {
      parsed.stderrTail = tail
      if (typeof entry.source === 'string') parsed.source = entry.source
    }
  }
  return parsed.stderrTail !== undefined || parsed.rpcErrorData !== undefined ? parsed : null
}

/** Whether "查看错误详情" has more than the bare message to show. */
export function hasRunFailureDiagnostic(failure: Pick<RunFailure, 'stderrTail' | 'rpcErrorData'>): boolean {
  return Boolean(failure.stderrTail?.trim() || failure.rpcErrorData?.trim())
}

/** Section title of each diagnostic source in "查看错误详情". */
export function diagnosticTitle(source: string | undefined): string {
  if (source === RPC_ERROR_DATA_SOURCE) return '后端错误数据（已脱敏，最多 4 KiB）：'
  const label = source ? SOURCE_LABELS[source] ?? source : ''
  return `后端 stderr 尾部（已脱敏，最多 4 KiB${label ? `；${label}` : ''}）：`
}

const SOURCE_LABELS: Record<string, string> = {
  prompt_failed: '本轮请求失败',
  backend_exited: '后端中途退出',
  session_failed: '会话打开失败',
  initialize: '后端启动失败',
}

/**
 * Text for "查看错误详情": the failure message, then the backend stderr tail
 * (already redacted by the Host) when the run carried one.
 */
export function runFailureDetailText(failure: Pick<RunFailure, 'code' | 'message' | 'stderrTail' | 'stderrTailSource' | 'rpcErrorData'>): string {
  const lines = [`${failure.code}: ${failure.message}`]
  if (failure.stderrTail && failure.stderrTail.trim()) {
    lines.push('', diagnosticTitle(failure.stderrTailSource), failure.stderrTail)
  }
  if (failure.rpcErrorData && failure.rpcErrorData.trim()) {
    lines.push('', diagnosticTitle(RPC_ERROR_DATA_SOURCE), failure.rpcErrorData)
  }
  return lines.join('\n')
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

/**
 * A0: a historical Kernel Run found unfinished at startup is interrupted with
 * this code (reason 「执行路径已退役」) and never resumed.
 */
export const EXECUTOR_ROUTE_RETIRED_CODE = 'executor_route_retired'
export const EXECUTOR_ROUTE_RETIRED_HINT = '已中断，可在新 Run 中继续'

function failureFor(
  kind: RunFailureKind,
  code?: string | null,
  message?: string | null,
): RunFailure {
  if (code === EXECUTOR_ROUTE_RETIRED_CODE) {
    const reason = message && message.length > 0 ? message : '执行路径已退役'
    return { kind, code, message: `${reason}。${EXECUTOR_ROUTE_RETIRED_HINT}` }
  }
  return {
    kind,
    code: code && code.length > 0 ? code : UNKNOWN_CODE,
    message: message && message.length > 0
      ? message
      : kind === 'failed' ? FAILED_MESSAGE : INTERRUPTED_MESSAGE,
  }
}

function withDiagnostic(failure: RunFailure, diagnostic: ParsedDiagnostic | null): RunFailure {
  if (!diagnostic) return failure
  const next: RunFailure = { ...failure }
  if (diagnostic.stderrTail !== undefined) {
    next.stderrTail = diagnostic.stderrTail
    if (diagnostic.source) next.stderrTailSource = diagnostic.source
  }
  if (diagnostic.rpcErrorData !== undefined) next.rpcErrorData = diagnostic.rpcErrorData
  return next
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
  // The stderr tail lives only in the terminal event payload; the persisted
  // run row (which wins for code / message) never carries it.
  const diagnostics = new Map<string, ParsedDiagnostic>()

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
    const failure = failureFor(kind, payload.code, payload.message)
    const diagnostic = diagnosticOf(payload)
    if (diagnostic) diagnostics.set(event.runId, diagnostic)
    failures.set(event.runId, withDiagnostic(failure, diagnostic))
  }

  // Persisted terminal runs override whatever the event stream suggested.
  for (const run of input.persistedRuns ?? []) {
    const kind = kindForStatus(run.status)
    if (kind) {
      cancelled.delete(run.id)
      failures.set(run.id, withDiagnostic(failureFor(kind, run.errorCode, run.errorMessage), diagnostics.get(run.id) ?? null))
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
