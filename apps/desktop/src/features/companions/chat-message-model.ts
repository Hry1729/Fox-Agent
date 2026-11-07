import type { RunEventRecord } from '@/features/conversations/model/types'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { projectRuntimeGroups } from '@/features/chat/runtime-process-groups'
import { splitAssistantContent } from '@/features/chat/timeline/fox-assistant-content'

export interface ContactReply {
  id: string
  text: string
  createdAt: number
}

export function contactReplies(message: FoxThreadMessage, events: readonly RunEventRecord[]): ContactReply[] {
  const source = message.source
  if (!source) return []
  const replies = source.replyMessages ?? source.kernelMessages ?? [source]
  return replies.flatMap(reply => {
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
    return text.trim() ? [{ id: reply.id, text, createdAt: reply.createdAt }] : []
  })
}
