const INTERNAL_PLANNING_PATTERN = /^(?:the user\s+(?:wants|asked|is asking|needs|provided|would like)\b|we\s+(?:need|should|must|have to)\b|i\s+(?:need|should|must|have to)\b|i(?:'ll| will)\s+(?:first|now|check|inspect|read|write|create|use|look|verify|figure|determine|handle|respond|explain|run)\b|let me\b|let's\b|looking at\s+(?:my|the)\b|now i\s+(?:have|need|can|should|will)\b|first,?\s+i\b|actually,?\b|wait,?\b|so i\s+(?:need|should|will|can)\b|the (?:task|request)\s+(?:is|asks|requires)\b)/i
const INTERNAL_PLANNING_CONTEXT_PATTERN = /^this is (?:the )?(?:\*\*)?(?:first|second|third|fourth|fifth|\w+)(?:\*\*)? (?:round|pass|attempt)(?:\*\*)? of (?:tool )?testing\b/i
const USER_FACING_ANSWER_PATTERN = /^(?:好的|好嘞|好，|当然|可以|没问题|已经|完成|以下是|这里是|结果如下|脚本已|文件已|我已经|我为你|done\b|sure\b|certainly\b|here(?:'s| is| are)\b|i(?:'ve| have)\s+(?:created|updated|finished|completed)\b|(?:created|updated|completed)\b|#{1,6}\s|```)/i
const INLINE_USER_FACING_ANSWER_PATTERN = /(?:好的|好嘞|好，|当然|可以|没问题|已经|完成|以下是|这里是|结果如下|脚本已|文件已|我已经|我为你)/u
const REASONING_TAIL_CHARS = 160
const STARTUP_CLASSIFICATION_CHARS = 220

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
  const match = text.match(/^\s*<(think|thinking|analysis)>[\s\S]*?<\/\1>\s*/i)
  if (!match) return null
  const reasoning = match[0]
    .replace(/^\s*<(?:think|thinking|analysis)>/i, '')
    .replace(/<\/(?:think|thinking|analysis)>\s*$/i, '')
    .trim()
  const answer = text.slice(match[0].length).trimStart()
  return { reasoning, answer }
}

function looksLikeUserFacingAnswer(text) {
  return USER_FACING_ANSWER_PATTERN.test(text) || /^[\u3400-\u9fff]/u.test(text)
}

function looksLikeInternalPlanning(text) {
  return INTERNAL_PLANNING_PATTERN.test(text) || INTERNAL_PLANNING_CONTEXT_PATTERN.test(text)
}

function answerStartInUnit(unit) {
  if (looksLikeUserFacingAnswer(unit.trimmed)) return unit.start
  if (!looksLikeInternalPlanning(unit.trimmed)) return null
  const match = INLINE_USER_FACING_ANSWER_PATTERN.exec(unit.trimmed)
  return match && match.index > 0 ? unit.start + match.index : null
}

function mixedAnswerStart(text) {
  const tagged = text.match(/^\s*<(think|thinking|analysis)>[\s\S]*?<\/\1>\s*/i)
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
      if (!split.reasoning) return message
      return {
        ...message,
        content: [
          { type: 'thinking', thinking: split.reasoning },
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
        { ...block, text: split.answer },
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
  let agentEnded = false
  let pendingText = ''
  let textMode = 'undecided'
  let reasoningEmittedUntil = 0
  let answerEmittedUntil = 0
  const providerReasoningByIndex = new Map()

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
      if (/^<(?:think|thinking|analysis)>/i.test(first) || internalSignals >= 2) {
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
          emit('tool.started', { toolCallId: event.toolCallId, tool: event.toolName, input: event.args })
          break
        case 'tool_execution_update':
          emit('tool.updated', { toolCallId: event.toolCallId, tool: event.toolName, update: event.partialResult })
          break
        case 'tool_execution_end':
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
            if (usage) emit('usage.updated', { inputTokens: usage.input ?? 0, outputTokens: usage.output ?? 0, totalTokens: usage.totalTokens ?? 0 })
            if (event.message.stopReason === 'error') {
              emit('message.completed')
              finished = true
              emit('run.failed', { code: 'provider.request_failed', message: event.message.errorMessage || 'The model request failed.' })
            } else if (event.message.stopReason === 'aborted') {
              emit('message.completed')
              finished = true
              emit('run.cancelled')
            }
          }
          break
        case 'agent_end':
          if (!finished) {
            ensureMessageStarted()
            emit('message.completed')
            if (deferCompletion) {
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
      finished = true
      emit('run.failed', { code: 'runtime.pi_failed', message: error instanceof Error ? error.message : String(error) })
    },
    finish() {
      if (!deferCompletion || finished || !agentEnded) return
      finished = true
      emit('run.completed')
    },
  }
}
