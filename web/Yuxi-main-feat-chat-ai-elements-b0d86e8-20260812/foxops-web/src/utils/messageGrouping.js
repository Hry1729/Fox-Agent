import MessageProcessor from './messageProcessor.js'
import { enrichTaskToolCalls } from '../components/ToolCallingResult/toolRegistry.js'

const hasAssistantError = (message) =>
  Boolean(
    message?.error_type ||
      message?.extra_metadata?.error_type ||
      message?.isStoppedByUser ||
      message?.extra_metadata?.is_stopped_by_user
  )

const defaultEnrichToolCalls = (message) => enrichTaskToolCalls(message?.tool_calls)

const appendPart = (parts, part) => {
  if (!part) return
  const previous = parts.at(-1)

  // 相邻的同类片段可以安全合并；跨工具调用的正文必须保留原始顺序。
  if (previous?.type === part.type && part.type === 'tool-group') {
    previous.toolCalls.push(...part.toolCalls)
    return
  }

  if (
    previous?.type === part.type &&
    (part.type === 'text' || part.type === 'reasoning') &&
    previous.message === part.message
  ) {
    previous.content = `${previous.content}${part.content}`
    return
  }

  parts.push(part)
}

const buildAssistantTurn = (messages, firstIndex, enrichToolCalls) => {
  const parts = []

  messages.forEach((message) => {
    const { content, reasoningContent } = MessageProcessor.parseAssistantMessageBody(message)

    if (reasoningContent) {
      appendPart(parts, { type: 'reasoning', content: reasoningContent, message })
    }
    if (content) {
      appendPart(parts, { type: 'text', content, message })
    }

    const toolCalls = enrichToolCalls(message)
    if (toolCalls.length > 0) {
      appendPart(parts, { type: 'tool-group', toolCalls: [...toolCalls], message })
    }

    if (hasAssistantError(message)) {
      appendPart(parts, { type: 'error', message })
    }
  })

  if (parts.length === 0) return null

  const representative = [...messages]
    .reverse()
    .find((message) => message?.id || MessageProcessor.parseAssistantMessageBody(message).content)
    || messages.at(-1)

  return {
    type: 'assistant-turn',
    key: `assistant-turn-${representative?.id || firstIndex}`,
    messages,
    message: representative,
    parts,
    sourceIndex: firstIndex
  }
}

// 每个 human 消息开始一轮；同一轮中的 AI 正文、推理与工具调用合并到一个助手 Message 中。
export const getConversationDisplayItems = (
  conv,
  { enrichToolCalls = defaultEnrichToolCalls } = {}
) => {
  if (!Array.isArray(conv?.messages) || conv.messages.length === 0) return []

  const items = []
  let assistantMessages = []
  let assistantStartIndex = -1

  const flushAssistantTurn = () => {
    if (assistantMessages.length > 0) {
      const turn = buildAssistantTurn(assistantMessages, assistantStartIndex, enrichToolCalls)
      if (turn) items.push(turn)
    }
    assistantMessages = []
    assistantStartIndex = -1
  }

  conv.messages.forEach((message, index) => {
    if (message.type === 'ai' || message.type === 'AIMessageChunk') {
      if (assistantStartIndex < 0) assistantStartIndex = index
      assistantMessages.push(message)
      return
    }

    flushAssistantTurn()
    items.push({
      type: 'message',
      key: message.id || `message-${index}`,
      message,
      sourceIndex: index
    })
  })

  flushAssistantTurn()
  return items
}
