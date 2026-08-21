/**
 * @typedef {'user' | 'assistant' | 'system'} ElementsRole
 */

/**
 * @typedef {'text' | 'reasoning' | 'tool-group' | 'sources' | 'artifacts' | 'error' | 'attachments' | 'image'} ElementsMessagePartType
 */

/**
 * @typedef {{
 *   type: ElementsMessagePartType
 *   payload: Record<string, unknown>
 * }} ElementsMessagePart
 */

/**
 * @typedef {{
 *   id: string
 *   role: ElementsRole
 *   parts: ElementsMessagePart[]
 *   meta: {
 *     sender?: string
 *     time?: string
 *     avatar?: string
 *     modelName?: string
 *     msgId?: string
 *     feedback?: unknown
 *   }
 * }} ElementsMessageView
 */

/**
 * 将 BubbleList 的显示项转换为 AI Elements 可消费的消息结构。
 *
 * @param {any[]} items
 * @returns {ElementsMessageView[]}
 */
export function toElementsMessages(items) {
  return items.map((item, index) => {
    /** @type {ElementsMessagePart[]} */
    const parts = []

    if (Array.isArray(item._parts)) {
      item._parts.forEach((part) => {
        if (part?.type && part?.payload && typeof part.payload === 'object') {
          parts.push(part)
        }
      })
    }

    if (!item._parts && item._reasoning) {
      parts.push({
        type: 'reasoning',
        payload: {
          reasoning: item._reasoning
        }
      })
    }

    if (!item._parts && item._type === 'tool-group') {
      parts.push({
        type: 'tool-group',
        payload: {
          toolCalls: item._toolCalls || []
        }
      })
    }

    if (!item._parts && item.content) {
      parts.push({
        type: 'text',
        payload: {
          content: item.content
        }
      })
    }

    const hasSources =
      (Array.isArray(item._sources) && item._sources.length > 0) ||
      Boolean(item._sources?.knowledgeChunks?.length || item._sources?.webSources?.length)
    if (!item._parts && hasSources) {
      parts.push({
        type: 'sources',
        payload: {
          sources: item._sources
        }
      })
    }

    if (Array.isArray(item._artifacts) && item._artifacts.length > 0) {
      parts.push({
        type: 'artifacts',
        payload: {
          artifacts: item._artifacts
        }
      })
    }

    if (Array.isArray(item._userAttachments) && item._userAttachments.length > 0) {
      parts.push({
        type: 'attachments',
        payload: {
          attachments: item._userAttachments
        }
      })
    }

    if (item._imageContent) {
      parts.push({
        type: 'image',
        payload: {
          content: item._imageContent,
          mime: item._imageMime
        }
      })
    }

    if (item._errorType || item._errorMessage || item._isStoppedByUser) {
      parts.push({
        type: 'error',
        payload: {
          errorType: item._errorType,
          errorMessage: item._errorMessage,
          isStoppedByUser: item._isStoppedByUser
        }
      })
    }

    return {
      id: String(item.key ?? item._msgId ?? index),
      role: item.placement === 'end' ? 'user' : 'assistant',
      parts,
      meta: {
        sender: item._sender,
        time: item._time,
        avatar: item._avatar,
        modelName: item._modelName,
        msgId: item._msgId,
        feedback: item._feedback
      }
    }
  })
}
