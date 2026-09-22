type JsonRecord = Record<string, unknown>

export type ExecutionReceiptKind = 'process' | 'file' | 'unknown'
export type ExecutionStartState = 'started' | 'not_started' | 'unknown'
export type ExecutionDisplayState = 'started' | 'not_started' | 'started_outcome_unknown' | 'start_unknown'

export type ExecutionReceiptPresentation = {
  kind: ExecutionReceiptKind
  startState: ExecutionStartState
  displayState: ExecutionDisplayState
  statusLabel: string
  stage: string | null
  controlPlane: string | null
  externalEffect: string | null
  code: string | null
  rawCodes: string[]
  knownRefusal: boolean
  uncertain: boolean
  replayAllowed: boolean
  requiresAttention: boolean
}

const uncertainCodes = new Set(['kernel.uncertain_execution', 'job.interrupted_unknown'])
const uncertainStates = new Set(['uncertain', 'unknown'])
const uncertainStages = new Set(['interrupted', 'file_recovery_required', 'file_indeterminate'])
const processStages = new Set(['admitted', 'job_created', 'launch_confirmed', 'interrupted'])

function record(value: unknown): JsonRecord | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as JsonRecord
    : null
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null
}

function startState(value: unknown): ExecutionStartState | null {
  if (value === true || value === 'true') return 'started'
  if (value === false || value === 'false') return 'not_started'
  if (value === 'unknown') return 'unknown'
  return null
}

function executionKind(value: unknown, stage: string | null): ExecutionReceiptKind {
  if (value === 'process' || value === 'file') return value
  if (stage?.startsWith('file_')) return 'file'
  if (stage && processStages.has(stage)) return 'process'
  return 'unknown'
}

function subjectLabel(kind: ExecutionReceiptKind, state: ExecutionStartState) {
  if (kind === 'file') {
    if (state === 'not_started') return '文件操作未开始'
    if (state === 'unknown') return '文件操作是否开始未知'
    return '文件操作已开始'
  }
  if (kind === 'process') {
    if (state === 'not_started') return '进程未启动'
    if (state === 'unknown') return '进程是否启动未知'
    return '进程已启动'
  }
  if (state === 'not_started') return '执行未开始'
  if (state === 'unknown') return '是否已执行未知'
  return '执行已开始'
}

/**
 * Projects the durable execution facts carried by a tool result. The nested
 * ExecutionReceipt is authoritative; direct fields are a compatibility path
 * for older persisted results and pre-admission refusals.
 */
export function executionReceiptPresentation(resultValue: unknown): ExecutionReceiptPresentation | null {
  const result = record(resultValue)
  if (!result) return null
  const details = record(result.details) ?? result
  const canonical = record(details.executionReceipt)
  const facts = canonical ?? details
  const started = startState(facts.executionStarted)
  if (!started) return null

  const stage = text(facts.stage)
  const kind = executionKind(facts.kind, stage)
  const controlPlane = text(facts.controlPlane)
  const externalEffect = text(canonical ? facts.externalEffect : facts.externalEffect ?? facts.sideEffectState)
  const canonicalReason = text(canonical?.reasonCode)
  const directReason = text(details.reasonCode)
  const error = record(details.error) ?? record(result.error)
  const transportCode = text(error?.code)
  const codeAlias = text(facts.codeAlias)
  const primaryCode = canonical ? canonicalReason ?? transportCode : directReason ?? transportCode

  const uncertain = started !== 'not_started' && (
    started === 'unknown'
    || uncertainStates.has(controlPlane ?? '')
    || uncertainStates.has(externalEffect ?? '')
    || uncertainStages.has(stage ?? '')
  )
  const aliasesDescribeSameFact = uncertain
    && primaryCode !== null
    && uncertainCodes.has(primaryCode)
    && (codeAlias === null || uncertainCodes.has(codeAlias))
  const code = aliasesDescribeSameFact ? 'kernel.uncertain_execution' : primaryCode
  const rawCodes = [...new Set([primaryCode, codeAlias].filter((value): value is string => value !== null))]
  const refusalCode = canonical ? canonicalReason : directReason ?? transportCode
  const knownRefusal = started === 'not_started'
    && refusalCode !== null
    && !uncertainCodes.has(refusalCode)
  const completedWithoutExternalOperation = kind === 'process'
    && stage === 'admitted'
    && started === 'not_started'
    && controlPlane === 'committed'
    && externalEffect === 'none'
    && !knownRefusal
  const displayState: ExecutionDisplayState = started === 'unknown'
    ? 'start_unknown'
    : uncertain
      ? 'started_outcome_unknown'
      : started === 'not_started'
        ? 'not_started'
        : 'started'

  let statusLabel = subjectLabel(kind, started)
  if (completedWithoutExternalOperation) statusLabel = '调用已完成（未启动外部操作）'
  else if (knownRefusal) statusLabel += ' · 已拒绝'
  if (uncertain) statusLabel += ' · 禁止重放'
  if (displayState === 'started_outcome_unknown') {
    statusLabel = `${subjectLabel(kind, started)} · 结果未知 · 禁止重放`
  } else if (kind === 'file' && started === 'started' && !uncertain
    && (stage === 'file_committed' || externalEffect === 'committed')) {
    statusLabel = '文件更改已提交'
  }

  return {
    kind,
    startState: started,
    displayState,
    statusLabel,
    stage,
    controlPlane,
    externalEffect,
    code,
    rawCodes,
    knownRefusal,
    uncertain,
    replayAllowed: uncertain ? false : facts.allowReplay === true,
    requiresAttention: knownRefusal || uncertain,
  }
}
