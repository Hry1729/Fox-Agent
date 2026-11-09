import { useMemo } from 'react'
import { MessagePrimitive, useAuiState } from '@assistant-ui/react'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { useFoxThread } from '@/features/chat/thread/fox-thread-context'
import { FoxCopyMessageButton, FoxMarkUnreadButton } from '@/features/chat/thread/fox-message-actions'
import { copyMessageResponse } from '@/components/assistant-ui/elements/message-actions'
import { StreamingText } from '@/features/chat/components/StreamingText'
import { RuntimeArtifacts } from '@/features/chat/runtime-artifacts'
import { resolveFinalArtifactReference } from '@/features/chat/final-artifact-references'
import { ErrorPrompt } from '@/features/chat/demo/fox-demo-prompts'
import { formatClockTime } from '@/lib/time-format'
import { notify as toast } from '@/features/notifications'
import { ContactWriteCards } from './chat-cards'
import { ContactDecisionCards } from './chat-decisions'
import { contactReplies } from './chat-message-model'
import { ContactAvatar } from './chat-controls'
import { contacts } from './api'
import { showContactError, useContacts } from './context'
import './contact-chat.css'

export function ContactAssistantMessage() {
  const fox = useFoxThread()
  const contactsCtx = useContacts()
  const contact = contactsCtx?.current
  const message = useAuiState(state => state.message.metadata?.custom?.fox as FoxThreadMessage | undefined)
  const lastInRun = useAuiState(state => {
    const current = state.message.metadata?.custom?.fox as FoxThreadMessage | undefined
    return state.thread.messages.findLast(item => item.role === 'assistant'
      && (item.metadata?.custom?.fox as FoxThreadMessage | undefined)?.runId === current?.runId)?.id === state.message.id
  })
  // Committed bubbles own the identity once the run settles. While the run is
  // live, event-derived replies that already match a persisted bubble take its
  // durable identity and delivery order; only drafts and replaceable previews
  // stay display-only until delivery.
  const persisted = message?.runId ? fox.contactBubblesByRun?.get(message.runId) : undefined
  const committed = !message?.isActiveRun ? persisted : undefined
  const replies = useMemo(() => {
    if (!message) return []
    if (committed?.length) return committed.map(bubble => ({ id: bubble.id, text: bubble.content, createdAt: bubble.deliveredAt, deliveryOrder: bubble.deliveryOrder as number | undefined }))
    const byId = new Map((persisted ?? []).map(bubble => [bubble.id, bubble]))
    const delivered = contactReplies(message, message.runId ? fox.eventsByRun.get(message.runId) ?? [] : [])
    const previews = message.isActiveRun && message.runId ? fox.contactKernelPreviewsByRun?.get(message.runId) ?? [] : []
    return [
      ...delivered.map(reply => {
        const bubble = byId.get(reply.id)
        return bubble
          ? { id: bubble.id, text: bubble.content, createdAt: bubble.deliveredAt, deliveryOrder: bubble.deliveryOrder as number | undefined }
          : { ...reply, deliveryOrder: undefined as number | undefined }
      }),
      ...previews
        .filter(reply => !byId.has(reply.id))
        .map(reply => ({ ...reply, deliveryOrder: undefined as number | undefined })),
    ]
  }, [message, fox.eventsByRun, fox.contactKernelPreviewsByRun, committed, persisted])
  if (!message || !fox.contactChat) return null
  const artifacts = message.content.flatMap(part => part.type === 'data-fox-artifacts' ? part.data : [])
  return <div id={`fox-turn-${message.id}`} data-turn-message-id={message.id} className="fox-turn-anchor">
    <MessagePrimitive.Root className="fox-thread-msg is-assistant fox-contact-run" onClickCapture={event => {
      const anchor = event.target instanceof Element ? event.target.closest('a') : null
      const href = anchor?.getAttribute('href') ?? ''
      if (!href.startsWith('#fox-artifact=')) return
      event.preventDefault()
      event.stopPropagation()
      const artifact = resolveFinalArtifactReference(href, artifacts)
      if (artifact) fox.onOpenArtifact?.(artifact)
      else toast.error('文件记录尚未找到')
    }}>
      {contact && <ContactAvatar contact={contact} />}
      <div className="fox-contact-run-body">
        {replies.map(reply => <section className="fox-contact-message" key={reply.id} data-contact-message-id={reply.id}
          data-bubble-id={reply.deliveryOrder != null ? reply.id : undefined}
          data-delivery-order={reply.deliveryOrder != null ? reply.deliveryOrder : undefined}>
          <div className="fox-contact-bubble"><StreamingText text={reply.text} streaming={false} className="fox-answer-response" /></div>
          <div className="fox-contact-message-meta">
            <FoxCopyMessageButton onCopy={() => copyMessageResponse(reply.text)} />
            {reply.deliveryOrder != null && contact && <FoxMarkUnreadButton onMark={() => contacts.markUnread(contact.id, reply.id).then(() => contactsCtx?.refresh()).catch(showContactError)} />}
            <time dateTime={new Date(reply.createdAt).toISOString()}>{formatClockTime(reply.createdAt)}</time>
          </div>
        </section>)}
      {lastInRun && <>
        <RuntimeArtifacts artifacts={artifacts.filter(artifact => artifact.declaredAt != null)} onOpenArtifact={fox.onOpenArtifact} />
        {message.runId && <ContactDecisionCards runId={message.runId} />}
        <ContactWriteCards runId={message.runId} />
        {message.failure && <ErrorPrompt failure={message.failure} />}
        {message.cancelled && <p className="fox-contact-ended" role="status">已停止，发出的消息已保留</p>}
      </>}
      </div>
    </MessagePrimitive.Root>
  </div>
}
