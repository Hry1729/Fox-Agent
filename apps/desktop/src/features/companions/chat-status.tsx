import { useMemo } from 'react'
import type { ConversationDetail } from '@/features/conversations/model/types'
import { presentConversationActivity } from '@/features/chat/run-activity-presentation'
import { pendingRuntimeApprovals, pendingRuntimeQuestion } from '@/features/conversations/model/pending-interactions'
import { ContactAvatar } from './chat-controls'
import { useContacts } from './context'
import './contact-chat.css'

export function ContactWorkStatus({ detail, conversationId }: { detail: ConversationDetail | null; conversationId: string | null }) {
  const contact = useContacts()?.current
  const activity = useMemo(() => presentConversationActivity(detail, conversationId), [detail, conversationId])
  if (!contact || contact.conversationId !== conversationId || detail?.conversation.id !== conversationId) return null
  const waiting = pendingRuntimeApprovals(detail).some(approval => approval.runId === activity.runId)
    || pendingRuntimeQuestion(detail)?.runId === activity.runId
  if (!activity.active && !waiting) return null
  const waitingForUser = waiting || activity.animation === 'listening'
  const label = waitingForUser ? '等你确认' : activity.label === '正在调用工具' ? '正在处理' : activity.label
  return <div className="fox-contact-work-status" data-working={!waitingForUser} data-run-id={activity.runId ?? ''}>
    <ContactAvatar contact={contact} />
    <span role="status" aria-live="polite">{label}</span>
  </div>
}
