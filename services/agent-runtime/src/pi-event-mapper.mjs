const INTERNAL_PLANNING_PATTERN = /^(?:the user\s+(?:wants|asked|is asking|needs|provided|would like)\b|we\s+(?:need|should|must|have to)\b|i\s+(?:need|should|must|have to)\b|i(?:'ll| will)\s+(?:first|now|check|inspect|read|write|create|use|look|verify|figure|determine|handle|respond|explain|run)\b|let me\b|let's\b|looking at\s+(?:my|the)\b|now i\s+(?:have|need|can|should|will)\b|first,?\s+i\b|actually,?\b|wait,?\b|so i\s+(?:need|should|will|can)\b|the (?:task|request)\s+(?:is|asks|requires)\b)/i
const INTERNAL_PLANNING_CONTEXT_PATTERN = /^this is (?:the )?(?:\*\*)?(?:first|second|third|fourth|fifth|\w+)(?:\*\*)? (?:round|pass|attempt)(?:\*\*)? of (?:tool )?testing\b/i
const INTERNAL_PLANNING_ZH_PATTERN = /^(?:我(?:先|来|再|现在|需要|得|要|会先|准备|去|应该|必须|把|按守则|直接调用)|先(?:确认|看看|检查|读取|分析|验证|对照|创建|列出|处理|提议)|现在(?:去|先|来|需要|掌握|确认)|接下来|让我(?:先|再|来|看看|确认|验证|检查|读取|分析)|好，?(?:工作目录|现在|先|接下来)|好的，?本次测试|本次测试要测|测试设计|当前对话(?:没有|尚无)|目标(?:已经|已)(?:提议|提交)|快照里|Host\b|Gate\b|task_create_many\b|goal_(?:propose|complete)\b|work_snapshot_get\b|对照表|关键确认|两个观察|最后再|不过本任务)/u
const USER_FACING_ANSWER_PATTERN = /^(?:好的|好嘞|当然|没问题|已经|完成|以下是|这里是|结果如下|结论(?:如下|是)?[：:]?|下面是|最终结果|汇总完毕|请你确认|脚本已|文件已|我已经|我为你|done\b|sure\b|certainly\b|here(?:'s| is| are)\b|i(?:'ve| have)\s+(?:created|updated|finished|completed)\b|(?:created|updated|completed)\b|#{1,6}\s|```)/i
const INLINE_USER_FACING_ANSWER_PATTERN = /(?:好的|好嘞|当然|没问题|已经|完成|以下是|这里是|结果如下|结论(?:如下|是)?[：:]?|下面是|最终结果|汇总完毕|请你确认|脚本已|文件已|我已经|我为你)/u
const REASONING_TAIL_CHARS = 160
const STARTUP_CLASSIFICATION_CHARS = 220
const USAGE_COMPONENT_FIELDS = Object.freeze([
  ['input', 'inputTokens'],
  ['output', 'outputTokens'],
  ['cacheRead', 'cacheReadTokens'],
  ['cacheWrite', 'cacheWriteTokens'],
])

function usageContractError(kind, detail) {
  return new Error(`[runtime.usage.${kind}] ${detail}`)
}

function normalizeUsageCounter(value, field) {
  if (value === undefined) return 0
  if (!Number.isSafeInteger(value) || value < 0) {
    throw usageContractError('invalid', `${field} must be a finite non-negative safe integer.`)
  }
  return value
}

function addUsageCounters(left, right, field) {
  if (right > Number.MAX_SAFE_INTEGER - left) {
    throw usageContractError('overflow', `${field} exceeds Number.MAX_SAFE_INTEGER.`)
  }
  return left + right
}

function sumUsageCounters(values, field) {
  return values.reduce((total, value) => addUsageCounters(total, value, field), 0)
}

function normalizeUsageStep(usage) {
  if (!usage || typeof usage !== 'object' || Array.isArray(usage)) {
    throw usageContractError('invalid', 'provider usage must be an object.')
  }
  const normalized = Object.fromEntries(USAGE_COMPONENT_FIELDS.map(([source, target]) => [
    target,
    normalizeUsageCounter(usage[source], source),
  ]))
  const componentTotal = sumUsageCounters(
    USAGE_COMPONENT_FIELDS.map(([, target]) => normalized[target]),
    'provider component total',
  )
  const reportedTotal = normalizeUsageCounter(usage.totalTokens, 'totalTokens')
  return { ...normalized, totalTokens: Math.max(reportedTotal, componentTotal) }
}

function accumulateUsage(current, step) {
  const next = Object.fromEntries(USAGE_COMPONENT_FIELDS.map(([, field]) => [
    field,
    addUsageCounters(current[field], step[field], `cumulative ${field}`),
  ]))
  const componentTotal = sumUsageCounters(
    USAGE_COMPONENT_FIELDS.map(([, field]) => next[field]),
    'cumulative component total',
  )
  const billedTotal = addUsageCounters(current.totalTokens, step.totalTokens, 'cumulative totalTokens')
  return { ...next, totalTokens: Math.max(billedTotal, componentTotal) }
}

function textUnits(text) {
  const units = []
  const pattern = /[^\r\n]+(?:\r?\n+|$)/g
  let match
  while ((match = pattern.exec(text)) !== null) {
    const content = match[0]
    const trimmed = content.trim()
    if (trimmed) units.push({ start: match.index, end: match.index + content.length, trimmed })
  }
  return units
}

function splitTaggedThinking(text) {
  const tagged = text.match(/^\s*<((?:mm:)?(?:think|thinking|analysis))>[\s\S]*?<\/\1>\s*/i)
  if (tagged) {
    const reasoning = tagged[0]
      .replace(/^\s*<(?:mm:)?(?:think|thinking|analysis)>/i, '')
      .replace(/<\/(?:mm:)?(?:think|thinking|analysis)>\s*$/i, '')
      .trim()
    return { reasoning, answer: text.slice(tagged[0].length).trimStart() }
  }

  // Some compatible providers omit the opening marker but still emit the
  // closing MiniMax marker. Treat the strongly signalled prefix as private.
  const closingOnly = text.match(/^\s*([\s\S]*?)<\/(?:mm:)?(?:think|thinking|analysis)>\s*/i)
  if (!closingOnly) return null
  return {
    reasoning: closingOnly[1].trim(),
    answer: text.slice(closingOnly[0].length).trimStart(),
  }
}

function looksLikeUserFacingAnswer(text) {
  return USER_FACING_ANSWER_PATTERN.test(text)
}

function looksLikeInternalPlanning(text) {
  return INTERNAL_PLANNING_PATTERN.test(text)
    || INTERNAL_PLANNING_CONTEXT_PATTERN.test(text)
    || INTERNAL_PLANNING_ZH_PATTERN.test(text)
}

function answerStartInUnit(unit) {
  if (looksLikeUserFacingAnswer(unit.trimmed)) return unit.start
  if (!looksLikeInternalPlanning(unit.trimmed)) return null
  const match = INLINE_USER_FACING_ANSWER_PATTERN.exec(unit.trimmed)
  return match && match.index > 0 ? unit.start + match.index : null
}

function mixedAnswerStart(text) {
  const tagged = text.match(/^\s*<((?:mm:)?(?:think|thinking|analysis))>[\s\S]*?<\/\1>\s*/i)
    || text.match(/^\s*[\s\S]*?<\/(?:mm:)?(?:think|thinking|analysis)>\s*/i)
  if (tagged) return tagged[0].length

  const units = textUnits(text)
  if (units.length < 2 || !looksLikeInternalPlanning(units[0].trimmed)) return null
  const internalIndexes = units.flatMap((unit, index) => looksLikeInternalPlanning(unit.trimmed) ? [index] : [])
  if (internalIndexes.length < 2) return null
  const lastInternalIndex = internalIndexes.at(-1)
  const embeddedAnswer = answerStartInUnit(units[lastInternalIndex])
  if (embeddedAnswer !== null) return embeddedAnswer
  for (const unit of units.slice(lastInternalIndex + 1)) {
    const answerStart = answerStartInUnit(unit)
    if (answerStart !== null) return answerStart
  }
  return null
}

/**
 * Some OpenAI-compatible providers place private planning and the final answer in
 * the same text field. Split only a strongly signalled planning preamble; ordinary
 * answers containing phrases such as "Let me explain" must remain untouched.
 */
export function splitMixedAssistantText(text) {
  if (typeof text !== 'string' || !text.trim()) return { reasoning: '', answer: text || '' }

  const tagged = splitTaggedThinking(text)
  if (tagged) return tagged

  const units = textUnits(text)
  if (units.length < 3 || !looksLikeInternalPlanning(units[0].trimmed)) {
    return { reasoning: '', answer: text }
  }

  const internalIndexes = units.flatMap((unit, index) => looksLikeInternalPlanning(unit.trimmed) ? [index] : [])
  if (internalIndexes.length < 2) return { reasoning: '', answer: text }

  const answerStart = mixedAnswerStart(text)
  // A stopped turn can contain only private planning when the model fails to
  // produce a final answer. Keeping the answer empty is safer than publishing it.
  if (answerStart === null) return { reasoning: text.trim(), answer: '' }

  const reasoning = text.slice(0, answerStart).trimEnd()
  const answer = text.slice(answerStart).trimStart()
  return reasoning && answer ? { reasoning, answer } : { reasoning: '', answer: text }
}

export function sanitizeAssistantHistory(messages) {
  if (!Array.isArray(messages)) return []
  return messages.map((message) => {
    if (message?.role !== 'assistant') return message

    if (typeof message.content === 'string') {
      const split = splitMixedAssistantText(message.content)
      return {
        ...message,
        content: [
          ...(split.reasoning ? [{ type: 'thinking', thinking: split.reasoning }] : []),
          ...(split.answer ? [{ type: 'text', text: split.answer }] : []),
        ],
      }
    }

    if (!Array.isArray(message.content)) return message

    const hasToolCall = message.content.some((block) => block?.type === 'toolCall')
    const content = message.content.flatMap((block) => {
      if (block?.type !== 'text' || !block.text) return [block]
      if (hasToolCall) return [{ type: 'thinking', thinking: block.text }]

      const split = splitMixedAssistantText(block.text)
      if (!split.reasoning) return [block]
      return [
        { type: 'thinking', thinking: split.reasoning },
        ...(split.answer ? [{ ...block, text: split.answer }] : []),
      ]
    })
    return { ...message, content }
  })
}

function knowledgeSources(result) {
  const candidates = [result?.details?.results, result?.details?.details?.results, result?.results]
  const results = candidates.find(Array.isArray) || []
  return results.flatMap((item, index) => {
    if (!item || typeof item !== 'object') return []
    const metadata = item.metadata && typeof item.metadata === 'object' ? item.metadata : {}
    const pageValue = item.page ?? item.page_number ?? metadata.page ?? metadata.page_number ?? metadata.page_num
    const page = Number(pageValue)
    const chunkId = String(item.chunk_id || metadata.chunk_id || item.id || '')
    const anchor = String(item.anchor || metadata.anchor || metadata.section || metadata.heading || '')
    return [{
      id: String(item.id || metadata.chunk_id || `${item.kb_id || 'knowledge'}-${index + 1}`),
      title: String(metadata.source || metadata.title || item.file_id || '知识库来源'),
      knowledgeBaseId: String(item.kb_id || result?.details?.kb_id || ''),
      documentId: String(item.file_id || metadata.file_id || ''),
      excerpt: String(item.content || '').slice(0, 320),
      chunkId,
      page: Number.isInteger(page) && page > 0 ? page : undefined,
      anchor,
      metadata,
    }]
  })
}

export function createPiEventMapper(emit, options = {}) {
  const deferCompletion = options.deferCompletion === true
  let messageStarted = false
  let finished = false
  let cancelled = false
  let agentEnded = false
  let pendingTerminal = null
  let pendingText = ''
  let textMode = 'undecided'
  let reasoningEmittedUntil = 0
  let answerEmittedUntil = 0
  const providerReasoningByIndex = new Map()
  const accountedUsageEvents = new WeakSet()
  let cumulativeUsage = {
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    totalTokens: 0,
  }

  function ensureMessageStarted() {
    if (messageStarted) return
    messageStarted = true
    emit('message.started', { role: 'assistant', kind: 'text' })
  }

  function resetTextTurn() {
    pendingText = ''
    textMode = 'undecided'
    reasoningEmittedUntil = 0
    answerEmittedUntil = 0
    providerReasoningByIndex.clear()
  }

  function emitProviderReasoning(contentIndex, delta, providerField = null) {
    if (!delta) return
    const index = Number.isInteger(contentIndex) ? contentIndex : 0
    providerReasoningByIndex.set(index, `${providerReasoningByIndex.get(index) || ''}${delta}`)
    emit('reasoning.delta', { delta, source: 'provider', providerField })
  }

  function reconcileProviderReasoning(message) {
    if (!Array.isArray(message?.content)) return
    message.content.forEach((block, index) => {
      if (block?.type !== 'thinking') return
      const complete = typeof block.thinking === 'string' ? block.thinking : ''
      if (!complete) return
      const emitted = providerReasoningByIndex.get(index) || ''
      if (complete.startsWith(emitted)) {
        emitProviderReasoning(index, complete.slice(emitted.length), block.thinkingSignature || null)
      } else if (!emitted.includes(complete)) {
        emitProviderReasoning(index, complete, block.thinkingSignature || null)
      }
    })
  }

  function emitReasoningUntil(end, source = 'mixed_text') {
    if (end <= reasoningEmittedUntil) return
    const delta = pendingText.slice(reasoningEmittedUntil, end)
    reasoningEmittedUntil = end
    if (delta) emit('reasoning.delta', { delta, source })
  }

  function emitAnswerFrom(start) {
    const from = Math.max(start, answerEmittedUntil || start)
    if (from >= pendingText.length) return
    const delta = pendingText.slice(from)
    answerEmittedUntil = pendingText.length
    if (delta) emit('message.delta', { delta })
  }

  function streamPendingText() {
    if (!pendingText) return
    const units = textUnits(pendingText)
    const first = units[0]?.trimmed ?? pendingText.trimStart()
    if (textMode === 'undecided') {
      const internalSignals = units.filter((unit) => looksLikeInternalPlanning(unit.trimmed)).length
      if (/^<(?:mm:)?(?:think|thinking|analysis)>/i.test(first) || internalSignals >= 2) {
        textMode = 'reasoning'
      } else if (looksLikeInternalPlanning(first)) {
        return
      } else if (looksLikeUserFacingAnswer(first)) {
        textMode = 'answer'
      } else if (/^this(?:\s|$)/i.test(first) && pendingText.length < STARTUP_CLASSIFICATION_CHARS && units.length < 2) {
        return
      } else {
        textMode = 'answer'
      }
    }

    if (textMode === 'answer') {
      emitAnswerFrom(0)
      return
    }

    const answerStart = mixedAnswerStart(pendingText)
    if (answerStart !== null) {
      emitReasoningUntil(answerStart)
      textMode = 'answer'
      emitAnswerFrom(answerStart)
      return
    }

    // Retain a short tail so a boundary phrase split across chunks can still be
    // classified before it is published to the work-process timeline.
    emitReasoningUntil(Math.max(reasoningEmittedUntil, pendingText.length - REASONING_TAIL_CHARS))
  }

  function flushPendingText(message) {
    if (!pendingText) return
    const hasToolCall = Array.isArray(message?.content)
      && message.content.some((block) => block?.type === 'toolCall')
    // Providers sometimes put pre-tool narration in the normal text channel.
    // A tool-calling turn is process output; only the final non-tool turn is the answer.
    if (hasToolCall) {
      if (answerEmittedUntil === 0) emitReasoningUntil(pendingText.length, 'tool_turn')
    } else {
      const split = splitMixedAssistantText(pendingText)
      if (split.reasoning) {
        const answerStart = split.answer ? pendingText.indexOf(split.answer) : pendingText.length
        emitReasoningUntil(answerStart >= 0 ? answerStart : pendingText.length)
      }
      if (split.answer) {
        const answerStart = pendingText.indexOf(split.answer)
        emitAnswerFrom(answerStart >= 0 ? answerStart : 0)
      } else if (!split.reasoning && answerEmittedUntil === 0) {
        emitAnswerFrom(0)
      }
    }
    resetTextTurn()
  }

  return {
    handle(event) {
      switch (event?.type) {
        case 'message_start':
          if (event.message?.role === 'assistant') {
            ensureMessageStarted()
            resetTextTurn()
          }
          break
        case 'message_update': {
          const update = event.assistantMessageEvent
          if (update?.type === 'text_delta' && update.delta) {
            ensureMessageStarted()
            pendingText += update.delta
            streamPendingText()
          } else if (update?.type === 'thinking_delta' && update.delta) {
            const thinkingBlock = update.partial?.content?.[update.contentIndex]
            emitProviderReasoning(update.contentIndex, update.delta, thinkingBlock?.thinkingSignature || null)
          }
          break
        }
        case 'tool_execution_start':
          // Pi emits start/end even for a name it rejects as "not found".
          // That is not a Host tool execution. Keep the SDK error in the model
          // transcript, without advertising an undeclared Runtime capability.
          if (options.isToolRegistered && !options.isToolRegistered(event.toolName)) {
            emit('run.phase', { phase: 'tool.rejected', code: 'runtime.tool_not_registered', tool: event.toolName })
            break
          }
          emit('tool.started', { toolCallId: event.toolCallId, tool: event.toolName,
            input: options.prepareToolInput?.(event.toolName, event.args) ?? event.args })
          break
        case 'tool_execution_update':
          if (options.isToolRegistered && !options.isToolRegistered(event.toolName)) break
          emit('tool.updated', { toolCallId: event.toolCallId, tool: event.toolName, update: event.partialResult })
          break
        case 'tool_execution_end':
          if (options.isToolRegistered && !options.isToolRegistered(event.toolName)) break
          if (event.toolName === 'search_knowledge') {
            for (const source of knowledgeSources(event.result)) emit('source.added', source)
          }
          emit('tool.completed', { toolCallId: event.toolCallId, tool: event.toolName, result: event.result, isError: Boolean(event.isError) })
          break
        case 'message_end':
          if (event.message?.role === 'assistant') {
            ensureMessageStarted()
            reconcileProviderReasoning(event.message)
            flushPendingText(event.message)
            const usage = event.message.usage
            if (usage !== undefined && !accountedUsageEvents.has(event)) {
              const nextUsage = accumulateUsage(cumulativeUsage, normalizeUsageStep(usage))
              emit('usage.updated', nextUsage)
              cumulativeUsage = nextUsage
              accountedUsageEvents.add(event)
            }
            if (cancelled) {
              // A user/host cancellation aborts the in-flight provider request, which
              // surfaces as stopReason 'error'/'aborted'. Cancellation is the authority:
              // never record an abort-induced provider error as a failed run.
              pendingTerminal = { type: 'run.cancelled' }
            } else if (event.message.stopReason === 'error') {
              pendingTerminal = {
                type: 'run.failed',
                code: 'provider.request_failed',
                message: event.message.errorMessage || 'The model request failed.',
              }
            } else if (event.message.stopReason === 'aborted') {
              pendingTerminal = { type: 'run.cancelled' }
            }
          }
          break
        case 'auto_retry_start':
          pendingTerminal = null
          emit('run.retrying', {
            attempt: event.attempt,
            maxAttempts: event.maxAttempts,
            delayMs: event.delayMs,
            message: event.errorMessage,
          })
          emit('run.phase', { phase: 'recovering', attempt: event.attempt })
          break
        case 'auto_retry_end':
          emit('run.retry.completed', {
            success: Boolean(event.success),
            attempt: event.attempt,
            finalError: event.finalError,
          })
          if (event.success) emit('run.phase', { phase: 'model_streaming', attempt: event.attempt + 1 })
          break
        case 'compaction_start':
          emit('context.compaction.started', { reason: event.reason })
          emit('run.phase', { phase: 'compacting' })
          break
        case 'compaction_end':
          emit('context.compaction.completed', {
            reason: event.reason,
            aborted: Boolean(event.aborted),
            willRetry: Boolean(event.willRetry),
            errorMessage: event.errorMessage,
          })
          if (!event.aborted) emit('run.phase', { phase: 'model_streaming' })
          break
        case 'agent_end':
          if (event.willRetry) {
            pendingTerminal = null
            emit('run.phase', { phase: 'recovering' })
            break
          }
          if (!finished) {
            ensureMessageStarted()
            emit('message.completed')
            if (pendingTerminal) {
              finished = true
              const terminal = pendingTerminal
              pendingTerminal = null
              emit(terminal.type, terminal.type === 'run.failed'
                ? { code: terminal.code, message: terminal.message }
                : {})
            } else if (deferCompletion) {
              agentEnded = true
            } else {
              finished = true
              emit('run.completed')
            }
          }
          break
        default:
          break
      }
    },
    fail(error) {
      if (finished) return
      if (cancelled) {
        // Cancellation wins over any late failure (aborted provider stream, host
        // request rejection, or Pi surfacing the abort as an error terminal).
        finished = true
        pendingTerminal = null
        emit('run.cancelled')
        return
      }
      finished = true
      const pending = pendingTerminal
      pendingTerminal = null
      emit('run.failed', pending?.type === 'run.failed'
        ? { code: pending.code, message: pending.message }
        : {
            code: typeof error?.code === 'string' && error.code.trim()
              ? error.code
              : 'runtime.pi_failed',
            message: error instanceof Error ? error.message : String(error),
          })
    },
    cancel() {
      cancelled = true
      if (finished) return
      // A cancellation arriving before the terminal agent event pre-declares the
      // outcome; the abort-induced provider 'error' terminal must not override it.
      pendingTerminal = { type: 'run.cancelled' }
      finished = true
      emit('run.cancelled')
    },
    finish() {
      if (!deferCompletion || finished || !agentEnded) return
      finished = true
      emit('run.completed')
    },
  }
}
