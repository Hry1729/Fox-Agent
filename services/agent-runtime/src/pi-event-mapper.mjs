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

const THINKING_OPEN_TAG = /^\s*<((?:mm:)?(?:think|thinking|analysis))>/i
const THINKING_TAG_NAMES = ['think', 'thinking', 'analysis', 'mm:think', 'mm:thinking', 'mm:analysis']

/**
 * Reasoning is whatever the provider marks as reasoning: its reasoning field,
 * or explicit thinking tags in the text channel. Plain answer text is never
 * reclassified by wording, in any language (MD-1).
 */
export function splitMixedAssistantText(text) {
  if (typeof text !== 'string' || !text.trim()) return { reasoning: '', answer: text || '' }
  return splitTaggedThinking(text) ?? { reasoning: '', answer: text }
}

function couldStillOpenThinkingTag(text) {
  const trimmed = text.trimStart().toLowerCase()
  if (!trimmed) return true
  if (!trimmed.startsWith('<')) return false
  return THINKING_TAG_NAMES.some((name) => `<${name}>`.startsWith(trimmed))
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
  const sources = []
  function collect(value, reference = {}, depth = 0) {
    if (!value || typeof value !== 'object' || depth > 4) return
    if (value.details) return collect(value.details, reference, depth + 1)
    if (value.result) return collect(value.result, value.reference ?? reference, depth + 1)
    const results = [value.items, value.results, value.chunks].find(Array.isArray) ?? []
    for (const [index, item] of results.entries()) {
      if (!item || typeof item !== 'object') continue
      if (item.result) { collect(item, reference, depth + 1); continue }
      const metadata = item.sourceMetadata ?? item.metadata ?? {}
      const knowledgeBaseId = String(item.knowledgeBaseId || item.kb_id || metadata.knowledgeBaseId || value.knowledgeBaseId || value.kb_id || reference.id || '')
      const documentId = String(item.documentId || item.file_id || metadata.documentId || metadata.file_id || value.documentId || '')
      if (!knowledgeBaseId || !documentId) continue
      const source = [item.source, metadata.source, value.source, reference.source].find(kind => kind === 'local' || kind === 'remote')
      const pageValue = item.page ?? item.page_number ?? metadata.pageNumber ?? metadata.page ?? metadata.page_number ?? metadata.page_num
      const page = Number(pageValue)
      const chunkId = String(item.chunkId || item.chunk_id || metadata.chunkId || metadata.chunk_id || item.id || '')
      const anchor = String(item.anchor || metadata.anchor || metadata.section || metadata.heading || '')
      sources.push({
        id: source === 'local' ? `local:${knowledgeBaseId}:${chunkId || `${documentId}-${index + 1}`}` : String(item.id || chunkId || `${knowledgeBaseId}-${index + 1}`),
        title: String(metadata.displayName || metadata.title || (metadata.source !== 'local' && metadata.source !== 'remote' ? metadata.source : '') || value.document?.displayName || item.file_id || '知识库来源'),
        knowledgeBaseId,
        documentId,
        excerpt: String(item.content || '').slice(0, 320),
        chunkId,
        page: Number.isInteger(page) && page > 0 ? page : undefined,
        anchor,
        metadata,
        ...(source === 'local' || source === 'remote' ? { source } : {}),
      })
    }
  }
  collect(result)
  return sources
}

export function createPiEventMapper(emit, options = {}) {
  const deferCompletion = options.deferCompletion === true
  let messageStarted = false
  let finished = false
  let cancelled = false
  let agentEnded = false
  let pendingTerminal = null
  let deferredTerminal = null
  let lastAssistant = null
  let lastCompactionErrorMessage = null
  let pendingText = ''
  let textMode = 'undecided'
  let thinkingTagName = ''
  let answerBase = 0
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
    thinkingTagName = ''
    answerBase = 0
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
    if (textMode === 'undecided') {
      const open = pendingText.match(THINKING_OPEN_TAG)
      if (open) {
        textMode = 'reasoning'
        thinkingTagName = open[1]
        reasoningEmittedUntil = open[0].length
      } else if (couldStillOpenThinkingTag(pendingText)) {
        return
      } else {
        textMode = 'answer'
      }
    }

    if (textMode === 'answer') {
      emitAnswerFrom(answerBase)
      return
    }

    const closeTag = `</${thinkingTagName}>`
    const closeAt = pendingText.toLowerCase().indexOf(closeTag.toLowerCase(), reasoningEmittedUntil)
    if (closeAt >= 0) {
      emitReasoningUntil(closeAt, 'tagged_text')
      reasoningEmittedUntil = closeAt + closeTag.length
      textMode = 'answer'
      const rest = pendingText.slice(reasoningEmittedUntil)
      const answerStart = reasoningEmittedUntil + (rest.length - rest.trimStart().length)
      answerBase = answerStart
      emitAnswerFrom(answerBase)
      return
    }
    // Hold back a closing tag that may be split across chunks.
    emitReasoningUntil(Math.max(reasoningEmittedUntil, pendingText.length - closeTag.length), 'tagged_text')
  }

  function flushPendingText(message) {
    if (!pendingText) return { answerEmitted: false }
    const hasToolCall = Array.isArray(message?.content)
      && message.content.some((block) => block?.type === 'toolCall')
    // Providers sometimes put pre-tool narration in the normal text channel.
    // A tool-calling turn is process output; only the final non-tool turn is the answer.
    if (hasToolCall) {
      if (answerEmittedUntil === 0) emitReasoningUntil(pendingText.length, 'tool_turn')
    } else if (textMode === 'reasoning') {
      // An unclosed provider thinking tag: everything is provider-marked reasoning.
      emitReasoningUntil(pendingText.length, 'tagged_text')
    } else {
      emitAnswerFrom(answerBase)
    }
    const answerEmitted = answerEmittedUntil > 0
    resetTextTurn()
    return { answerEmitted }
  }

  function visibleAnswerInCompletedMessage(message) {
    if (Array.isArray(message?.content) && message.content.some((block) => block?.type === 'toolCall')) return false
    const text = Array.isArray(message?.content)
      ? message.content.filter((block) => block?.type === 'text' && typeof block.text === 'string').map((block) => block.text).join('')
      : typeof message?.content === 'string' ? message.content : ''
    if (!text.trim()) return false
    return Boolean(splitMixedAssistantText(text).answer.trim())
  }

  function finalTerminalFromLastAssistant() {
    const assistant = lastAssistant
    // A tool-only turn is a process step, not a final answer. The visible text
    // and toolCall facts come from the last completed assistant message.
    if (!assistant) {
      return {
        type: 'run.failed',
        code: 'model.empty_response',
        message: 'The model returned no visible answer.',
      }
    }
    if (assistant.stopReason === 'length') {
      return {
        type: 'run.failed',
        code: 'model.output_truncated',
        message: assistant.compactionErrorMessage
          ? `The model output was truncated after context compaction failed: ${assistant.compactionErrorMessage}`
          : 'The model output was truncated before a final answer.',
      }
    }
    if (assistant.hasToolCall || assistant.visibleText) return { type: 'run.completed' }
    return {
      type: 'run.failed',
      code: 'model.empty_response',
      message: 'The model returned no visible answer.',
    }
  }

  function emitTerminal(terminal) {
    if (!terminal) return
    if (terminal.type === 'run.failed') {
      emit('run.failed', { code: terminal.code, message: terminal.message })
      return
    }
    emit(terminal.type)
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
          if (!event.isError && (event.toolName === 'search_knowledge' || event.toolName === 'read_knowledge_document')) {
            for (const source of knowledgeSources(event.result)) emit('source.added', source)
          }
          emit('tool.completed', { toolCallId: event.toolCallId, tool: event.toolName, result: event.result, isError: Boolean(event.isError) })
          break
        case 'message_end':
          if (event.message?.role === 'assistant') {
            ensureMessageStarted()
            reconcileProviderReasoning(event.message)
            const flushed = flushPendingText(event.message)
            const hasToolCall = Array.isArray(event.message.content)
              && event.message.content.some((block) => block?.type === 'toolCall')
            // Record the last completed assistant turn. The final terminal is
            // derived only after `agent_end` says there will be no more retries.
            lastAssistant = {
              stopReason: event.message.stopReason ?? null,
              visibleText: flushed.answerEmitted || visibleAnswerInCompletedMessage(event.message),
              hasToolCall,
              compactionErrorMessage: lastCompactionErrorMessage,
            }
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
        case 'compaction_end': {
          const compactionFailed = Boolean(event.errorMessage)
            || (!event.aborted && !event.result)
          lastCompactionErrorMessage = compactionFailed
            ? (event.errorMessage || 'Context compaction completed without a result.')
            : null
          if (compactionFailed) {
            emit('context.compaction.failed', {
              reason: event.reason,
              aborted: Boolean(event.aborted),
              willRetry: Boolean(event.willRetry),
              errorMessage: lastCompactionErrorMessage,
            })
            break
          }
          emit('context.compaction.completed', {
            reason: event.reason,
            aborted: Boolean(event.aborted),
            willRetry: Boolean(event.willRetry),
            errorMessage: event.errorMessage,
          })
          if (!event.aborted) emit('run.phase', { phase: 'model_streaming' })
          break
        }
        case 'agent_end':
          if (event.willRetry) {
            pendingTerminal = null
            emit('run.phase', { phase: 'recovering' })
            break
          }
          if (!finished) {
            const terminal = pendingTerminal ?? finalTerminalFromLastAssistant()
            pendingTerminal = null
            if (deferCompletion) {
              // Keep the assistant message streamable until finish(). A deferred
              // Run may compact and retry after this agent_end, and completing the
              // message here makes the Host reject the retry's message.delta.
              agentEnded = true
              deferredTerminal = terminal
            } else {
              ensureMessageStarted()
              emit('message.completed')
              finished = true
              emitTerminal(terminal)
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
      const terminal = deferredTerminal ?? pendingTerminal ?? finalTerminalFromLastAssistant()
      deferredTerminal = null
      pendingTerminal = null
      ensureMessageStarted()
      emit('message.completed')
      emitTerminal(terminal)
    },
  }
}
