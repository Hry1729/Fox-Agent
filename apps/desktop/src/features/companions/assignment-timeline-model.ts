import type { CompanionAssignment } from '@/features/conversations/model/types'

export interface CommunicationAnchor { id: string; createdAt: number; runId?: string | null }
export interface CompanionCommunication {
  id: string
  assignment: CompanionAssignment
  direction: 'sent' | 'received'
  createdAt: number
  delivered: boolean
}
export interface CommunicationGroup {
  id: string
  beforeId: string | null
  threadId: string
  peerId: string | null
  peerName: string
  events: CompanionCommunication[]
}

export function groupCompanionCommunications(records: readonly CompanionAssignment[], anchors: readonly CommunicationAnchor[], conversationId: string, companionId?: string, oldestMessageAt: number | null = null): CommunicationGroup[] {
  const visible = anchors.toSorted((a, b) => a.createdAt - b.createdAt)
  const events = records.flatMap(assignment => {
    if (assignment.messages) return assignment.messages.map(message => ({
      ...message, assignment, delivered: message.direction === 'received' || assignment.deliveryStatus === 'delivered',
    }))
    const sent: CompanionCommunication = { id: `${assignment.id}:sent`, assignment, direction: 'sent', createdAt: assignment.createdAt, delivered: assignment.deliveryStatus === 'delivered' }
    const returned = assignment.reply !== 'none' && (assignment.status === 'completed' && Boolean(assignment.result) || Boolean(assignment.question))
    const resumedAt = assignment.resumeRunId ? visible.find(anchor => anchor.runId === assignment.resumeRunId)?.createdAt : undefined
    return returned ? [sent, { id: `${assignment.id}:received`, assignment, direction: 'received' as const,
      createdAt: Math.min(assignment.updatedAt, resumedAt ?? Infinity), delivered: true }] : [sent]
  }).filter(event => oldestMessageAt === null || event.createdAt >= oldestMessageAt
    || !['completed', 'failed', 'cancelled'].includes(event.assignment.status))
    .sort((a, b) => a.createdAt - b.createdAt || (a.direction === b.direction ? a.id.localeCompare(b.id) : a.direction === 'sent' ? -1 : 1))
  const groups: CommunicationGroup[] = []
  let anchorIndex = 0
  for (const event of events) {
    while (anchorIndex < visible.length && visible[anchorIndex].createdAt < event.createdAt) anchorIndex++
    const assignment = event.assignment
    const outgoing = assignment.sourceConversationId === conversationId || Boolean(companionId && companionId === assignment.sourceCompanionId)
    const peerId = outgoing ? assignment.recipientCompanionId : assignment.sourceCompanionId ?? null
    const peerName = outgoing ? assignment.recipientName : assignment.sourceName
    const beforeId = visible[anchorIndex]?.id ?? null
    const previous = groups.at(-1)
    const shown = outgoing ? event : { ...event, direction: event.direction === 'sent' ? 'received' as const : 'sent' as const }
    if (previous && previous.beforeId === beforeId && previous.threadId === assignment.recipientConversationId && previous.peerId === peerId
      && event.delivered && previous.events.every(item => item.delivered)) previous.events.push(shown)
    else groups.push({ id: event.id, beforeId, threadId: assignment.recipientConversationId, peerId, peerName, events: [shown] })
  }
  return groups
}
