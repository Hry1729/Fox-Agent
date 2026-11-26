import { createContext, useContext, useMemo, type ReactNode } from 'react'
import { useAuiState } from '@assistant-ui/react'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { useFoxThread } from '@/features/chat/thread/fox-thread-context'
import { useUserProfile } from '@/features/profile/user-profile'
import { useContacts } from './context'
import { ContactAvatar } from './chat-controls'
import { visibleContactReplies } from './chat-message-model'
import { AssignmentActions, useCompanionAssignments } from './assignment-thread'
import { groupCompanionCommunications, type CommunicationGroup } from './assignment-timeline-model'

const CommunicationContext = createContext<{
  groups: CommunicationGroup[]
  state: ReturnType<typeof useCompanionAssignments>
  conversationId: string | null
  user: { displayName: string; avatarRef: string }
} | null>(null)

export function CompanionCommunicationProvider({ conversationId, oldestMessageAt, children }: { conversationId: string | null; oldestMessageAt: number | null; children: ReactNode }) {
  const contacts = useContacts()
  const { profile } = useUserProfile(contacts?.userProfile)
  const fox = useFoxThread()
  const messages = useAuiState(state => state.thread.messages)
  const state = useCompanionAssignments(contacts?.openCoordination ? conversationId : null)
  const groups = useMemo(() => {
    const anchors = messages.flatMap(message => {
      const source = message.metadata?.custom?.fox as FoxThreadMessage | undefined
      if (message.role === 'user' || !fox.contactChat) return [{ id: message.id, createdAt: message.createdAt.getTime(), runId: source?.runId }]
      if (!source) return []
      const run = source.runId ?? source.id
      return visibleContactReplies(source, fox.eventsByRun.get(run) ?? [], fox.contactBubblesByRun?.get(run))
        .map(reply => ({ ...reply, runId: source.runId }))
    })
    return groupCompanionCommunications(state.records, anchors, conversationId ?? '', contacts?.current?.id, oldestMessageAt)
  }, [messages, state.records, conversationId, contacts?.current?.id, oldestMessageAt, fox.contactChat, fox.eventsByRun, fox.contactBubblesByRun])
  return <CommunicationContext.Provider value={{ groups, state, conversationId, user: { displayName: profile.name, avatarRef: profile.avatar } }}>{children}</CommunicationContext.Provider>
}

export function CompanionCommunicationBefore({ messageId = null }: { messageId?: string | null }) {
  const context = useContext(CommunicationContext)
  const contacts = useContacts()
  if (!context) return null
  const groups = context.groups.filter(group => group.beforeId === messageId)
  return <>
    {groups.map(group => <CompanionCommunicationLine key={group.id} group={group} />)}
    {messageId === null && <>
      {context.state.records.filter(assignment => assignment.sourceConversationId === context.conversationId || Boolean(contacts?.current && assignment.sourceCompanionId === contacts.current.id))
        .map(assignment => <AssignmentActions key={assignment.id} assignment={assignment} onAnswer={context.state.answer} onCancel={context.state.cancel} />)}
    </>}
  </>
}

export function CompanionCommunicationLine({ group }: { group: CommunicationGroup }) {
  const contacts = useContacts()
  const user = useContext(CommunicationContext)?.user
  const peer = group.peerId ? contacts?.list.companions.find(contact => contact.id === group.peerId) : user
  const name = peer?.displayName ?? group.peerName
  const event = group.events[0]
  const label = group.events.length > 1 ? `${group.events.length} 条消息往来：`
    : !event.delivered ? event.assignment.deliveryStatus === 'failed' ? '未能发送给' : '等待发送给'
      : event.direction === 'sent' ? '已发消息给' : '消息来自'
  return <div className="fox-communication-line" data-communication-count={group.events.filter(item => item.delivered).length}>
    <span className="fox-communication-label">{label}</span>
    <button type="button" className="fox-assignment-chip" aria-label={`${label} ${name}，查看对话`}
      onClick={() => contacts?.openCoordination?.(group.threadId)}>
      <ContactAvatar contact={peer ?? { displayName: name, avatarRef: '' }} /><span className="fox-assignment-name">{name}</span>
    </button>
  </div>
}
