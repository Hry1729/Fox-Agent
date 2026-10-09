import type { RunEventRecord } from '@/features/conversations/model/types'
import { TERMINAL_RUN_EVENT_TYPES } from './turn-process-timing'

type JsonRecord = Record<string, unknown>

function record(value: unknown): JsonRecord | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as JsonRecord : null
}

/** Display only bounded diagnostic fields selected by the Host, never raw arguments. */
export function safeDiagnosticText(value: unknown, limit = 600): string | null {
  if (typeof value !== 'string' || !value.trim()) return null
  return value
    .replace(/\b(Bearer\s+)[^\s,;]+/gi, '$1[已隐藏]')
    .replace(/\b((?:api[_-]?key|token|password|secret|authorization)\s*[=:]\s*)[^\s,;]+/gi, '$1[已隐藏]')
    .replace(/(https?:\/\/)[^/\s@]+@/gi, '$1[已隐藏]@')
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/g, '')
    .trim().slice(0, limit)
}

function nonnegative(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null
}

function dispatchSequence(value: unknown): bigint | null {
  if (typeof value === 'number' && Number.isSafeInteger(value) && value > 0) return BigInt(value)
  if (typeof value === 'string' && /^[1-9]\d{0,18}$/.test(value)) return BigInt(value)
  return null
}

export interface ModelWaitingFact {
  phase: 'model_response' | 'context_compaction'
  requestId: string
  dispatchSeq: string
  startedAt: number
  elapsedMs: number
  remainingMs: number
  seq: number
}

const inactiveStates = new Set(['completed', 'cancelled', 'failed', 'interrupted', 'budget_exhausted', 'approval_expired', 'cancelling', 'waiting_approval', 'retry_scheduled'])
const responseEvents = new Set(['reasoning.delta', 'message.started', 'message.delta', 'message.completed', 'tool.started', 'tool.updated', 'tool.completed', 'context.compaction.completed', 'context.compaction.failed', 'run.retrying', 'run.waiting_approval'])

/** Presentation sequence and dispatch sequence have different domains. Never compare them. */
export function activeModelWaiting(events: readonly RunEventRecord[], options: { runId?: string; runState?: string | null } = {}): ModelWaitingFact | null {
  if (options.runState && inactiveStates.has(options.runState)) return null
  const runId = options.runId ?? events[0]?.runId
  const ordered = events.filter(item => item.runId === runId).sort((a, b) => a.seq - b.seq)
  if (ordered.some(item => TERMINAL_RUN_EVENT_TYPES.has(item.eventType))) return null
  let waiting: ModelWaitingFact | null = null
  let latestDispatch = 0n
  let requestOwner: string | null = null
  const closedRequests = new Set<string>()
  const closedDispatches = new Set<bigint>()
  for (const item of ordered) {
    if (item.eventType === 'run.model_waiting') {
      const phase = item.event.phase
      const requestId = typeof item.event.requestId === 'string' ? item.event.requestId : ''
      const dispatch = dispatchSequence(item.event.dispatchSeq)
      const startedAt = nonnegative(item.event.startedAt)
      const elapsedMs = nonnegative(item.event.elapsedMs)
      const remainingMs = nonnegative(item.event.remainingMs)
      if ((phase !== 'model_response' && phase !== 'context_compaction') || !requestId || requestId.length > 128
        || dispatch === null || startedAt === null || elapsedMs === null || remainingMs === null
        || item.event.outcomeKnown !== false || dispatch < latestDispatch || closedDispatches.has(dispatch)
        || closedRequests.has(`${dispatch}:${requestId}`)) continue
      // A different request cannot take ownership of the same dispatch.
      if (waiting && dispatch === latestDispatch && waiting.requestId !== requestId) continue
      if (dispatch === latestDispatch && requestOwner && requestOwner !== requestId) continue
      latestDispatch = dispatch
      requestOwner = requestId
      waiting = { phase, requestId, dispatchSeq: dispatch.toString(), startedAt, elapsedMs, remainingMs, seq: item.seq }
      continue
    }
    const dispatchBoundary = item.eventType === 'run.phase' && item.event.phase === 'request_sent'
      || item.eventType === 'context.compaction.dispatched'
    if (dispatchBoundary) {
      const dispatch = dispatchSequence(item.event.dispatchSeq)
      if (dispatch !== null && dispatch < latestDispatch) continue
      if (dispatch !== null && dispatch === latestDispatch) {
        const owner = typeof item.event.requestId === 'string' ? item.event.requestId : null
        if (owner) requestOwner = owner
        if (waiting && owner && waiting.requestId !== owner) waiting = null
        continue
      }
      if (waiting) {
        closedRequests.add(`${waiting.dispatchSeq}:${waiting.requestId}`)
        closedDispatches.add(BigInt(waiting.dispatchSeq))
      }
      waiting = null
      requestOwner = typeof item.event.requestId === 'string' ? item.event.requestId : null
      if (dispatch !== null) latestDispatch = dispatch
      continue
    }
    const phaseBoundary = item.eventType === 'run.phase' && ['preparing', 'streaming', 'finalizing'].includes(String(item.event.phase))
    if (phaseBoundary || item.eventType === 'context.compaction.started' || responseEvents.has(item.eventType)) {
      const dispatch = dispatchSequence(item.event.dispatchSeq)
      if (dispatch !== null && dispatch < latestDispatch) continue
      const requestId = typeof item.event.requestId === 'string' ? item.event.requestId : null
      if (waiting && dispatch === latestDispatch && requestId && requestId !== waiting.requestId) continue
      const compactionRetry = item.eventType === 'context.compaction.failed' && item.event.retryable === true
        && item.event.retryTimeline === 'next_attempt_inside_same_deadline' && item.event.outcomeKnown === true
      if (waiting) {
        closedRequests.add(`${waiting.dispatchSeq}:${waiting.requestId}`)
        if (!compactionRetry) closedDispatches.add(BigInt(waiting.dispatchSeq))
      }
      if (requestId && dispatch !== null) closedRequests.add(`${dispatch}:${requestId}`)
      if (dispatch !== null && item.eventType !== 'context.compaction.started' && !compactionRetry) closedDispatches.add(dispatch)
      waiting = null
      if (dispatch !== null) latestDispatch = dispatch > latestDispatch ? dispatch : latestDispatch
      requestOwner = null
    }
  }
  if (waiting && options.runState === 'compacting' && waiting.phase !== 'context_compaction') return null
  return waiting
}

/**
 * The user-facing sentence for a run that is waiting on the model.
 *
 * Deliberately short: the remaining execution budget, the request/dispatch
 * identity and the millisecond timings are internal accounting facts, not
 * something to show a person waiting for a reply. They stay in the durable
 * run record and in the background diagnostics.
 */
export function modelWaitingTitle(waiting: ModelWaitingFact): string {
  return waiting.phase === 'context_compaction' ? '正在整理上下文' : '正在等待回复'
}

const compactionCategories: Record<string, string> = {
  provider_unavailable: '模型服务不可用', invalid_result: '压缩结果无效', cancelled: '压缩已取消',
  transport_outcome_unknown: '传输结果未知', unknown: '原因类别未知',
  length: '响应达到长度上限', invalid_tool_arguments: '工具参数无效', truncated_tool_proposal: '工具提案不完整',
  protocol_invalid: '响应协议无效', stream_interrupted: '模型响应流中断', worker_exception: '模型请求处理异常',
  incomplete_response: '模型响应不完整', argument_validation_unavailable: '工具参数完整性无法核验',
}

/**
 * The user-facing line for a context compaction that did not finish.
 *
 * Only the fixed category sentence is shown. The category *code*, the retry
 * timeline, the failure classification, the HTTP status, the attempt count, the
 * millisecond timings and the redacted upstream text are internal accounting
 * and diagnostic facts: they stay in the durable events (and in the
 * user-initiated diagnostic export), never in a status line a person reads. The
 * failure itself is never hidden — an unknown outcome still says so.
 */
export function compactionFailurePresentation(event: RunEventRecord): { label: string } | null {
  if (event.eventType !== 'context.compaction.failed') return null
  const data = record(event.event.data) ?? event.event
  const category = typeof data.category === 'string' && Object.hasOwn(compactionCategories, data.category) ? data.category : 'unknown'
  const known = data.outcomeKnown === true
  return { label: `上下文整理未完成 · ${compactionCategories[category]}${known ? '' : ' · 结果未知'}` }
}

/**
 * The user-facing sentence for a failed call, chosen from the trusted category.
 *
 * A category is only ever the Host's own structured error-code contract
 * (`tool.*`, `kernel.*`, `mcp.*`) — never prose. The raw diagnostic the Host and
 * the model exchange (`details.underlyingMessage`, the refusal text inside a
 * policy-denial result, the connector's own body) stays in the durable record;
 * it is not transcribed into the interface, because it carries internal codes,
 * frozen-scope jargon and budget numbers that mean nothing to a person.
 */
type FailureCopy = { summary: string; hint?: string }

/** Wording a write-capable tool gets instead of the generic sentence. */
const writeFailureCopy: Record<string, FailureCopy> = {
  'tool.read_only_input': { summary: '本次未能写入文件', hint: '这个文件是只读材料，请把结果另存到别的文件' },
  'tool.permission_denied': { summary: '本次未能写入文件', hint: '当前权限不允许写入这个位置，请调整权限或换一个位置后重试' },
  // A policy refusal the Host did not sub-categorise: the sentence stays true
  // without claiming which rule refused it.
  'kernel.policy_denied': { summary: '本次未能写入文件', hint: '请确认写入位置在当前允许的范围内，或换一个位置后重试' },
  'tool.path_denied': { summary: '本次未能写入文件', hint: '这个位置不允许写入，请换一个位置' },
  'tool.file_conflict': { summary: '文件已被修改，未能保存', hint: '请重新读取后再保存' },
}

const localFailureCopy: Record<string, FailureCopy> = {
  'tool.read_only_input': { summary: '这个文件是只读材料', hint: '请改为读取它，或把结果另存到别的文件' },
  'tool.permission_denied': { summary: '当前权限不允许这次操作', hint: '如需继续，请调整权限后重试' },
  'kernel.policy_denied': { summary: '本次操作没有得到授权', hint: '如需继续，请在允许的范围内重试' },
  'tool.path_denied': { summary: '这个位置不允许访问', hint: '请换一个位置' },
  'tool.file_conflict': { summary: '文件已被修改', hint: '请重新读取后再保存' },
  'tool.invalid_input': { summary: '调用内容不符合要求', hint: '请检查后重试' },
  'tool.nonzero_exit': { summary: '命令没有成功执行' },
  'tool.start_failed': { summary: '命令无法启动' },
  // The Host stopped a tool that ran past its own execution bound.
  'tool.timed_out': { summary: '执行超时，已停止' },
  'tool.computation_timed_out': { summary: '数据处理超时，已停止' },
  'tool.cancelled': { summary: '已取消' },
  'tool.computation_syntax': { summary: '数据处理代码有语法错误' },
  'tool.computation_failed': { summary: '数据处理未能完成' },
  'tool.computation_runtime': { summary: '数据处理未能完成' },
  'tool.sandbox_unavailable': { summary: '当前环境不支持这个操作' },
  'tool.pdf_font_unavailable': { summary: 'PDF 生成缺少可用字体' },
  'tool.pdf_missing_glyph': { summary: 'PDF 生成缺少可用字形' },
  'tool.pdf_generation_failed': { summary: 'PDF 未能生成' },
  'tool.pdf_font_invalid': { summary: 'PDF 生成字体不可用' },
  'tool.resource_failed': { summary: '操作未能完成', hint: '可以稍后重试' },
  'tool.unknown': { summary: '操作未能完成', hint: '可以稍后重试' },
  'kernel.resource_failed': { summary: '操作未能完成', hint: '可以稍后重试' },
  'kernel.execution_failed': { summary: '操作未能完成', hint: '可以稍后重试' },
  // Never worded as "stopped": an uncertain execution is a fact about what is
  // *not* known, so the sentence says that and leaves the decision to the user.
  'kernel.uncertain_execution': { summary: '执行结果未知', hint: '未自动重试；请确认实际结果后再决定是否重新发起' },
}

/** Connector categories. Kept apart so a timeout or an unreachable connector is
 *  never reported as a local file or process failure — and never as "stopped". */
const remoteFailureCopy: Record<string, FailureCopy> = {
  'mcp.timed_out': { summary: '连接超时', hint: '请重试' },
  'mcp.connection_unavailable': { summary: '连接器暂时不可用', hint: '请稍后重试' },
  'mcp.configuration_changed': { summary: '连接器配置已变化', hint: '请重新连接后再试' },
  'mcp.remote_failure': { summary: '连接器调用未成功', hint: '请重试' },
}

/** No trusted category: a real but deliberately limited sentence. */
const unknownFailureCopy: FailureCopy = { summary: '本次操作未能完成', hint: '可以稍后重试' }

/**
 * The operation classes the Host names on an operation it identified itself.
 *
 * `file_write` comes from the managed file-write gate and `office_write` from the
 * built-in Office mutating gate. The class is a Host fact about an operation the
 * Host confirmed (its own tool identity, or the built-in Office `serverId` plus
 * that tool's mutating class), so it — and only it — decides the write wording.
 */
const writeOperationKinds = new Set(['file_write', 'office_write'])

function isWriteOperation(tool: string, details: JsonRecord | null): boolean {
  const kind = details?.operationKind
  if (typeof kind === 'string' && writeOperationKinds.has(kind)) return true
  // Direct classification by the Host's own tool identity. An operation *name*
  // never decides: an ordinary connector may expose a tool called
  // `office_create`, and a faithful copy of that name proves nothing about the
  // operation type.
  return tool === 'write_file' || tool === 'edit_file'
}

function trustedCategory(value: unknown, allowed: Record<string, unknown>): string | null {
  return typeof value === 'string' && Object.hasOwn(allowed, value) ? value : null
}

export interface ToolFailurePresentation {
  operation: string
  target: string | null
  summary: string
  hint: string | null
}

/**
 * Project one failed call into user-facing copy.
 *
 * `tool.completed`/`isError` still decide *that* the call failed; this only
 * decides how it reads. Nothing here treats a persisted result as proof that the
 * call executed.
 */
export function toolFailurePresentation(tool: string, result: unknown): ToolFailurePresentation {
  const output = record(result)
  const details = record(output?.details) ?? record(output?.error)
  const connectorTool = tool === 'call_mcp_tool' || tool === 'list_mcp_tools'
  // Who authored this result? Only Fox writes `diagnosticSource`, and the Host
  // writes it last at the execution boundary, so a remote connector body cannot
  // claim Host provenance — its Host-reserved keys are moved to
  // `connectorDetails` there. A connector result without the Host mark is never
  // read as a Host diagnosis, whatever codes its body carries.
  const source = typeof details?.diagnosticSource === 'string'
    ? details.diagnosticSource
    : typeof output?.diagnosticSource === 'string' ? output.diagnosticSource : null
  const hostAuthored = !connectorTool || source === 'host'
  // Display fields come from Host-authored facts only: a connector body must not
  // be able to name the operation or the target the interface shows.
  const trusted = hostAuthored ? details : null
  const operation = safeDiagnosticText(trusted?.operation, 128) ?? tool
  const target = safeDiagnosticText(trusted?.target, 256)
  // Structured categories only, in the order the Host establishes them: the code
  // it tagged on the refusal, then its error code, then the executor's own
  // underlying code.
  const categories = [details?.reasonCode, details?.errorCode, details?.underlyingCode]
  const localCategory = categories.map((value) => trustedCategory(value, localFailureCopy)).find(Boolean) ?? null
  // A connector call is worded with the connector vocabulary — unless the Host
  // authored the call's result *and* named a local category on it, which is what
  // a mutating Office write refused by the Host's own gate looks like.
  if (connectorTool && !(hostAuthored && localCategory)) {
    const category = categories.map((value) => trustedCategory(value, remoteFailureCopy)).find(Boolean) ?? 'mcp.remote_failure'
    const copy = remoteFailureCopy[category] ?? unknownFailureCopy
    return { operation, target, summary: copy.summary, hint: copy.hint ?? null }
  }
  const copy = (localCategory && isWriteOperation(tool, details) ? writeFailureCopy[localCategory] : undefined)
    ?? (localCategory ? localFailureCopy[localCategory] : undefined)
    ?? unknownFailureCopy
  return { operation, target, summary: copy.summary, hint: copy.hint ?? null }
}

/** A planned retry is not an executed retry; a later dispatch is separate evidence. */
export function runtimeStatusNotice(item: RunEventRecord, events: readonly RunEventRecord[]): string | null {
  if (item.eventType === 'run.retrying') return '已安排重试'
  if (item.eventType === 'run.retry.completed') return '重试已结束；结果以运行和工具记录为准'
  if (item.eventType === 'run.phase' && item.event.phase === 'request_sent'
    && events.some(previous => previous.runId === item.runId && previous.seq < item.seq && previous.eventType === 'run.retrying')) return '模型请求已发送'
  return null
}
