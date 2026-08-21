const getUserMessageContent = (message) => {
  if (!message || (message.type !== 'human' && message.role !== 'user')) return ''
  return typeof message.content === 'string' ? message.content.trim() : ''
}

export function resolveRetryUserMessage({ target, conversations = [], bubbleItems = [] } = {}) {
  const conversationIndex = Number(target?._convIdx)
  if (Number.isInteger(conversationIndex) && conversationIndex >= 0) {
    const messages = conversations[conversationIndex]?.messages
    if (Array.isArray(messages)) {
      const source = messages.find((message) => getUserMessageContent(message))
      if (source) return { content: getUserMessageContent(source), message: source }
    }
  }

  const targetIndex = bubbleItems.findIndex((item) => item?.key === target?.key)
  for (let index = targetIndex - 1; index >= 0; index -= 1) {
    const item = bubbleItems[index]
    if (item?.placement !== 'end') continue
    const content = typeof item.content === 'string' ? item.content.trim() : ''
    if (content) return { content, message: item }
  }

  return null
}
