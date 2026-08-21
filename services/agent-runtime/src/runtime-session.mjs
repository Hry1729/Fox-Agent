import { mkdir, readFile, rename, unlink, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'

export function createSessionState(runtimeSessionId, conversationId) {
  return {
    schemaVersion: 1,
    runtimeSessionId,
    conversationId,
    messages: [],
    updatedAt: new Date().toISOString(),
  }
}

export async function loadSessionState(sessionPath, expectedSessionId) {
  const session = JSON.parse(await readFile(sessionPath, 'utf8'))
  if (session?.schemaVersion !== 1 || session?.runtimeSessionId !== expectedSessionId) {
    throw new Error('The Pi runtime session is incompatible or belongs to another session.')
  }
  if (!Array.isArray(session.messages)) session.messages = []
  return session
}

export async function saveSessionState(sessionPath, session) {
  session.updatedAt = new Date().toISOString()
  await mkdir(dirname(sessionPath), { recursive: true })
  const temporaryPath = `${sessionPath}.${process.pid}.${Date.now()}.tmp`
  try {
    await writeFile(temporaryPath, JSON.stringify(session), 'utf8')
    await rename(temporaryPath, sessionPath)
  } catch (error) {
    await unlink(temporaryPath).catch(() => undefined)
    throw error
  }
}

export function normalizeHistory(messages) {
  if (!Array.isArray(messages)) return []
  return messages.flatMap((message) => {
    if (!message || !['user', 'assistant', 'toolResult'].includes(message.role)) {
      return []
    }
    if (message.role === 'toolResult') return [message]
    if (typeof message.content === 'string' || Array.isArray(message.content)) return [message]
    return []
  })
}

// Provider responses may add fields that another API rejects (for example
// OpenAI Responses rejecting an Anthropic/tool metadata field). Keep the
// durable transcript expressive, but send only the canonical Pi block shape.
export function sanitizeProviderHistory(messages) {
  return normalizeHistory(messages).map((message) => {
    if (message.role === 'toolResult') {
      return {
        role: 'toolResult',
        toolCallId: message.toolCallId,
        toolName: message.toolName,
        content: Array.isArray(message.content)
          ? message.content.filter((item) => item?.type === 'text' && typeof item.text === 'string')
              .map((item) => ({ type: 'text', text: item.text }))
          : String(message.content ?? ''),
        // Tool details are Fox/provider diagnostics, not model conversation
        // input. Replaying them can leak fields such as `namespace` into an
        // OpenAI Responses request.
        details: {},
        isError: Boolean(message.isError),
        timestamp: message.timestamp,
      }
    }
    if (typeof message.content === 'string') {
      return {
        role: message.role,
        content: message.role === 'assistant'
          ? (message.content ? [{ type: 'text', text: message.content }] : [])
          : message.content,
        timestamp: message.timestamp,
      }
    }
    const content = Array.isArray(message.content)
      ? message.content.flatMap((block) => {
          if (!block || typeof block !== 'object' || typeof block.type !== 'string') return []
          if (block.type === 'text' && typeof block.text === 'string') {
            return [{ type: 'text', text: block.text }]
          }
          if (block.type === 'thinking' && typeof block.thinking === 'string') {
            return [{
              type: 'thinking',
              thinking: block.thinking,
              ...(typeof block.thinkingSignature === 'string'
                ? { thinkingSignature: block.thinkingSignature }
                : {}),
            }]
          }
          if (block.type === 'toolCall' && typeof block.name === 'string') {
            return [{
              type: 'toolCall',
              ...(typeof block.id === 'string' ? { id: block.id } : {}),
              name: block.name,
              arguments: block.arguments ?? block.input ?? {},
            }]
          }
          if (block.type === 'image' && typeof block.data === 'string') {
            return [{
              type: 'image',
              data: block.data,
              ...(typeof block.mimeType === 'string' ? { mimeType: block.mimeType } : {}),
            }]
          }
          return []
        })
      : []
    return { role: message.role, content, timestamp: message.timestamp }
  })
}

function textProjection(messages) {
  return normalizeHistory(messages).flatMap((message) => {
    if (!['user', 'assistant'].includes(message.role)) return []
    if (typeof message.content === 'string') return [{ role: message.role, content: message.content }]
    if (!Array.isArray(message.content)) return []
    const content = message.content
      .filter((item) => item?.type === 'text')
      .map((item) => item.text)
      .join('')
    return [{ role: message.role, content }]
  })
}

export function transcriptFromSession(session, fallbackMessages) {
  const fallback = normalizeHistory(fallbackMessages)
  const restored = sanitizeProviderHistory(session?.messages)
  if (restored.length === 0) return fallback
  if (fallback.length === 0) return restored
  return JSON.stringify(textProjection(restored)) === JSON.stringify(textProjection(fallback))
    ? restored
    : fallback
}
