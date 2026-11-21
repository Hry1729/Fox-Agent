import { useMemo, type ReactNode } from 'react'
import { Conversation, ConversationContent } from '@/components/ai-elements/conversation'
import { Button } from '@/components/ui/button'
import { useUserProfile } from '@/features/profile/user-profile'
import { StreamingText } from '@/features/chat/components/StreamingText'
import { deriveFoxThreadMessages } from '@/features/chat/thread/convert-detail'
import { splitAssistantContent } from '@/features/chat/timeline/fox-assistant-content'
import type { CompanionAssignment, ConversationDetail } from '@/features/conversations/model/types'
import { ContactAvatar } from './chat-controls'
import { ContactDecisionExplanation } from './chat-decision'
import { contactReplies } from './chat-message-model'
import { contactGroupTimeLabel, contactTimeGroups } from './chat-time'
import { AssignmentActions, useCompanionAssignments } from './assignment-thread'
import { useContacts } from './context'
import './assignment-thread.css'

interface CoordinationEntry { id: string; createdAt: number; author: 'source' | 'recipient'; text: string; streaming?: boolean }

export function coordinationTranscriptEntries(records: readonly CompanionAssignment[], detail: ConversationDetail, streamingText = ''): CoordinationEntry[] {
  const oldest = detail.hasEarlierMessages ? Math.min(...detail.messages.map(message => message.createdAt)) : -Infinity
  const messages = deriveFoxThreadMessages({ detail, streamingText })
  const replies = messages.filter(message => message.role === 'assistant').flatMap(message => {
    if (records.length && records.every(record => record.messages !== undefined) && !message.isActiveRun) return []
    const events = detail.runtimeEvents.filter(event => event.runId === message.runId)
    const bubbles = contactReplies(message, events).map(reply => ({ ...reply, author: 'recipient' as const }))
    if (message.isActiveRun) {
      const text = splitAssistantContent(message.content.filter(part => part.type === 'text').map(part => part.text).join('\n')).answer
      if (text && !bubbles.some(bubble => bubble.text === text)) bubbles.push({ id: `${message.id}:preview`, text, createdAt: message.createdAt, author: 'recipient' })
    }
    return bubbles
  })
  const entries: CoordinationEntry[] = [...replies]
  for (const assignment of records) {
    if (assignment.messages) {
      for (const message of assignment.messages) {
        if (message.createdAt >= oldest && (message.direction === 'sent' || !entries.some(entry => entry.id === message.id || entry.text === message.text && entry.createdAt === message.createdAt))) {
          entries.push({ ...message, author: message.direction === 'sent' ? 'source' : 'recipient' })
        }
      }
      continue
    }
    if (assignment.createdAt >= oldest) entries.push({ id: `${assignment.id}:sent`, createdAt: assignment.createdAt, author: 'source', text: assignment.instructions || assignment.title })
    const hasReply = messages.some(message => message.role === 'assistant' && message.runId === assignment.recipientRunId)
    if (!hasReply && assignment.updatedAt >= oldest && (assignment.result || assignment.question)) entries.push({
      id: `${assignment.id}:received`, createdAt: assignment.updatedAt, author: 'recipient',
      text: assignment.question?.prompt ?? assignment.result!,
    })
  }
  return entries.sort((a, b) => a.createdAt - b.createdAt || (a.author === b.author ? a.id.localeCompare(b.id) : a.author === 'source' ? -1 : 1))
}

export function CoordinationTranscript({ detail, streamingText, approvalSlot, loadEarlierMessages }: {
  detail: ConversationDetail
  streamingText?: string
  approvalSlot?: ReactNode
  loadEarlierMessages?: () => Promise<void>
}) {
  const context = useContacts()
  const { profile } = useUserProfile(context?.userProfile)
  const { records, cancel } = useCompanionAssignments(detail.conversation.id)
  const thread = detail.coordination!
  const source = thread.sourceCompanionId ? context?.list.companions.find(contact => contact.id === thread.sourceCompanionId)
    ?? { displayName: thread.sourceName, avatarRef: '' } : { displayName: profile.name, avatarRef: profile.avatar }
  const recipient = context?.list.companions.find(contact => contact.id === thread.recipientCompanionId)
    ?? { displayName: records[0]?.recipientName ?? '伙伴', avatarRef: '' }
  const entries = useMemo(() => coordinationTranscriptEntries(records, detail, streamingText), [records, detail, streamingText])
  const times = useMemo(() => contactTimeGroups(entries), [entries])
  return <Conversation className="fox-coordination-transcript" initial="instant" resize="instant" aria-label={`${source.displayName}与${recipient.displayName}的消息往来`}>
    <ConversationContent className="fox-coordination-messages">
      {detail.hasEarlierMessages && <Button type="button" variant="ghost" className="fox-coordination-earlier" onClick={() => void loadEarlierMessages?.()}>加载更早消息</Button>}
      {entries.map(entry => {
        const author = entry.author === 'source' ? source : recipient
        return <div key={entry.id} className="fox-coordination-entry" data-message-author={entry.author}>
          {times.has(entry.id) && <div className="fox-contact-time-divider"><time dateTime={new Date(entry.createdAt).toISOString()}>{contactGroupTimeLabel(entry.createdAt)}</time></div>}
          <div className="fox-coordination-message"><ContactAvatar contact={author} /><div className="fox-coordination-message-body">
            <span className="fox-coordination-author">{author.displayName}</span>
            <div className="fox-coordination-bubble"><StreamingText text={entry.text} streaming={Boolean(entry.streaming)} /></div>
          </div></div>
        </div>
      })}
      {records.map(assignment => <AssignmentActions key={assignment.id} assignment={assignment} cancelOnly
        onAnswer={async () => {}} onCancel={cancel} />)}
      {approvalSlot && <div className="fox-coordination-message fox-coordination-approval">
        <ContactAvatar contact={recipient} />
        <div className="fox-coordination-message-body">
          <span className="fox-coordination-author">{recipient.displayName}</span>
          {entries.at(-1)?.author !== 'recipient' && <ContactDecisionExplanation>要继续这件事，我需要你处理下面的请求。</ContactDecisionExplanation>}
          {approvalSlot}
        </div>
      </div>}
    </ConversationContent>
  </Conversation>
}
