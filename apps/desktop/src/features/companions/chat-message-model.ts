import type { CompanionGuide, ContactBubbleRecord, RunEventRecord } from '@/features/conversations/model/types'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { projectRuntimeGroups } from '@/features/chat/runtime-process-groups'
import { splitAssistantContent } from '@/features/chat/timeline/fox-assistant-content'

export interface ContactReply {
  id: string
  text: string
  createdAt: number
  deliveryOrder?: number
  guide?: CompanionGuide
}

export function contactBubbleGuide(bubble: ContactBubbleRecord): CompanionGuide | undefined {
  const value = bubble.metadata?.companionGuide
  if (!value || typeof value !== 'object' || typeof value.title !== 'string' || !Array.isArray(value.options)) return undefined
  const options = value.options.filter(option => option && typeof option.label === 'string' && typeof option.message === 'string' && option.label.trim() && option.message.trim()).slice(0, 4)
  return options.length ? { title: value.title, options } : undefined
}

export function visibleContactReplies(message: FoxThreadMessage, events: readonly RunEventRecord[], persisted: ContactBubbleRecord[] = []): ContactReply[] {
  const project = (bubble: ContactBubbleRecord) => ({
    id: bubble.id, text: bubble.content, createdAt: bubble.deliveredAt,
    deliveryOrder: bubble.deliveryOrder, guide: contactBubbleGuide(bubble),
  })
  const projected = contactReplies(message, events)
  const replies = new Map(projected.map(reply => [reply.id, reply]))
  if (!message.isActiveRun && persisted.length && !persisted.some(bubble => replies.has(bubble.id))) {
    return persisted.map(project)
  }
  const sourceIds = new Set(replyMessages(message).map(reply => reply.id))
  for (const bubble of persisted) {
    if (!message.isActiveRun || replies.has(bubble.id) || sourceIds.has(bubble.messageId)) replies.set(bubble.id, project(bubble))
  }
  return [...replies.values()]
}

function replyMessages(message: FoxThreadMessage) {
  const source = message.source
  return source ? source.replyMessages ?? source.kernelMessages ?? [source] : []
}

export function contactReplies(message: FoxThreadMessage, events: readonly RunEventRecord[]): ContactReply[] {
  return replyMessages(message).flatMap(reply => {
    if (reply.kernelPreview || reply.status === 'streaming') return []
    if (reply.id === `assistant-${reply.runId}` && reply.runId) {
      // Thread context can advance a render before assistant-ui updates its message.
      const lastEventSeq = message.lastEventSeq
      const groups = projectRuntimeGroups(
        reply.runId,
        reply.content,
        lastEventSeq === undefined ? events : events.filter(event => event.seq <= lastEventSeq),
      )
      if (groups) return groups.flatMap(group => {
        if (group.kind !== 'response') return []
        const text = splitAssistantContent(group.text).answer
        return text.trim() ? [{ id: group.key, text, createdAt: reply.createdAt }] : []
      })
    }
    const text = splitAssistantContent(reply.content).answer
    if (!text.trim()) return []
    const prefix = `kernel-message:${reply.runId}:`
    const checkpoint = reply.runId && reply.id.startsWith(prefix) ? reply.id.slice(prefix.length) : null
    const id = checkpoint && /^[1-9]\d{0,18}$/.test(checkpoint)
      ? `${reply.runId}:kernel:${checkpoint}:response` : reply.id
    return [{ id, text, createdAt: reply.createdAt }]
  })
}
