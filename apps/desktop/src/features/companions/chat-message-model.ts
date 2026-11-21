import type { CompanionGuide, ContactBubbleRecord, RunEventRecord } from '@/features/conversations/model/types'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { projectRuntimeGroups } from '@/features/chat/runtime-process-groups'
import { splitAssistantContent } from '@/features/chat/timeline/fox-assistant-content'

export interface ContactReply {
  id: string
  text: string
  createdAt: number
}

export function contactBubbleGuide(bubble: ContactBubbleRecord): CompanionGuide | undefined {
  const value = bubble.metadata?.companionGuide
  if (!value || typeof value !== 'object' || typeof value.title !== 'string' || !Array.isArray(value.options)) return undefined
  const options = value.options.filter(option => option && typeof option.label === 'string' && typeof option.message === 'string' && option.label.trim() && option.message.trim()).slice(0, 4)
  return options.length ? { title: value.title, options } : undefined
}

export function visibleContactReplies(message: FoxThreadMessage, events: readonly RunEventRecord[], persisted: ContactBubbleRecord[] = [], previews: ContactReply[] = []): Array<ContactReply & { deliveryOrder?: number; guide?: CompanionGuide }> {
  const project = (bubble: ContactBubbleRecord) => ({
    id: bubble.id, text: bubble.content, createdAt: bubble.deliveredAt,
    deliveryOrder: bubble.deliveryOrder, guide: contactBubbleGuide(bubble),
  })
  if (!message.isActiveRun && persisted.length) return persisted.map(project)
  const replies = new Map<string, ContactReply & { deliveryOrder?: number; guide?: CompanionGuide }>()
  for (const reply of contactReplies(message, events)) replies.set(reply.id, reply)
  if (message.isActiveRun) {
    for (const preview of previews) if (!replies.has(preview.id)) replies.set(preview.id, preview)
  }
  const sourceIds = new Set(replyMessages(message).map(reply => reply.id))
  for (const bubble of persisted) {
    // The committed bubble can reach the React context before assistant-ui
    // finishes replacing its streaming message. Keep the same entry during that frame.
    if (replies.has(bubble.id) || sourceIds.has(bubble.messageId)) replies.set(bubble.id, project(bubble))
  }
  return [...replies.values()]
}

function replyMessages(message: FoxThreadMessage) {
  const source = message.source
  return source ? source.replyMessages ?? source.kernelMessages ?? [source] : []
}

export function contactReplies(message: FoxThreadMessage, events: readonly RunEventRecord[]): ContactReply[] {
  return replyMessages(message).flatMap(reply => {
    // Kernel previews are replaceable snapshots. Only committed replies become bubbles.
    if (reply.kernelPreview) return []
    if (reply.id === `assistant-${reply.runId}` && reply.runId) {
      const groups = projectRuntimeGroups(reply.runId, reply.content, events)
      if (groups) return groups.flatMap((group, index) => {
        if (group.kind !== 'response') return []
        if (message.isActiveRun && index === groups.length - 1 && reply.status === 'streaming') return []
        const text = splitAssistantContent(group.text).answer
        return text.trim() ? [{ id: group.key, text, createdAt: reply.createdAt }] : []
      })
    }
    if (reply.status === 'streaming') return []
    const text = splitAssistantContent(reply.content).answer
    if (!text.trim()) return []
    // A settled Kernel round keeps the durable bubble identity it had as a
    // live preview, so committing the run never remounts the bubble.
    const prefix = `kernel-message:${reply.runId}:`
    const checkpoint = reply.runId && reply.id.startsWith(prefix) ? reply.id.slice(prefix.length) : null
    const id = checkpoint && /^[1-9]\d{0,18}$/.test(checkpoint)
      ? `${reply.runId}:kernel:${checkpoint}:response` : reply.id
    return [{ id, text, createdAt: reply.createdAt }]
  })
}
