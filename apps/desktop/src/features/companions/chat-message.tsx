import { useMemo } from 'react'
import { MessagePrimitive, useAuiState } from '@assistant-ui/react'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { useFoxThread } from '@/features/chat/thread/fox-thread-context'
import { RuntimeArtifacts } from '@/features/chat/runtime-artifacts'
import { FinalArtifactContext, normalizeFinalArtifactLinks, resolveFinalArtifactReference } from '@/features/chat/final-artifact-references'
import { formatRunElapsed, runElapsedBounds } from '@/features/chat/turn-process-timing'
import { runFailureUserMessage } from '@/features/conversations/model/run-failure'
import { notify as toast } from '@/features/notifications'
import { ContactWriteCards } from './chat-cards'
import { ContactDecisionCards } from './chat-decisions'
import { visibleContactReplies } from './chat-message-model'
import { ContactTimeDivider } from './chat-time'
import { ContactReplyBubble } from './marks'
import { useContacts } from './context'
import { CompanionCommunicationBefore } from './assignment-timeline'
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
  const replies = useMemo(() => message ? visibleContactReplies(
    message,
    message.runId ? fox.eventsByRun.get(message.runId) ?? [] : [],
    fox.contactBubblesByRun?.get(message.runId ?? message.id),
  ) : [], [message, fox.eventsByRun, fox.contactBubblesByRun])
  if (!message || !fox.contactChat) return null
  const artifacts = message.content.flatMap(part => part.type === 'data-fox-artifacts' ? part.data : [])
  const deliveries = artifacts.filter(artifact => artifact.declaredAt != null)
  const hasReport = deliveries.some(artifact => artifact.status === 'ready'
    && (artifact.artifactType === 'companion_report' || /\.(?:md|markdown|docx|pdf|txt|html)$/iu.test(artifact.displayName)))
  const renderedReplies = replies.map(reply => ({ ...reply, text: normalizeFinalArtifactLinks(reply.text, artifacts) }))
  const retry = contactsCtx?.retryRun
  const { startedAt, endedAt } = runElapsedBounds(lastInRun && message.runId ? fox.eventsByRun.get(message.runId) ?? [] : [])
  return <FinalArtifactContext.Provider value={artifacts}><div id={`fox-turn-${message.id}`} data-turn-message-id={message.id} className="fox-turn-anchor">
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
      {/* SPEC item 9：不再渲染每条回复旁的模型头像——顶部胶囊已经有头像。 */}
      <div className="fox-contact-run-body">
        <ContactDecisionCards runId={lastInRun ? message.runId : null} replies={renderedReplies.map(reply => ({
          id: reply.id,
          time: reply.createdAt,
          content: <><CompanionCommunicationBefore messageId={reply.id} /><ContactTimeDivider messageId={reply.id} /><ContactReplyBubble contact={contact} reply={reply} hasReport={hasReport}
            initialMark={fox.contactBubbleMarks?.get(reply.id)} /></>,
        }))} />
      {lastInRun && <>
        <RuntimeArtifacts artifacts={deliveries} onOpenArtifact={fox.onOpenArtifact} />
        <ContactWriteCards runId={message.runId} />
        {message.failure && <p className="fox-contact-ended" role="status">
          {runFailureUserMessage(message.failure)}
          {retry && retry.id === message.runId && <button type="button" onClick={() => void retry.resume()}>继续处理</button>}
        </p>}
        {message.cancelled && <p className="fox-contact-ended" role="status">已停止，发出的消息已保留</p>}
        {!message.isActiveRun && !message.failure && !message.cancelled && renderedReplies.length > 0 && startedAt !== null && endedAt !== null
          && <small className="fox-contact-ended">用时 {formatRunElapsed(endedAt - startedAt)}</small>}
      </>}
      </div>
    </MessagePrimitive.Root>
  </div></FinalArtifactContext.Provider>
}
