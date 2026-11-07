import { useMemo } from 'react'
import { MessagePrimitive, useAuiState } from '@assistant-ui/react'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { useFoxThread } from '@/features/chat/thread/fox-thread-context'
import { FoxCopyMessageButton } from '@/features/chat/thread/fox-message-actions'
import { copyMessageResponse } from '@/components/assistant-ui/elements/message-actions'
import { StreamingText } from '@/features/chat/components/StreamingText'
import { RuntimeArtifacts } from '@/features/chat/runtime-artifacts'
import { resolveFinalArtifactReference } from '@/features/chat/final-artifact-references'
import { ErrorPrompt } from '@/features/chat/demo/fox-demo-prompts'
import { formatDateTime } from '@/lib/time-format'
import { notify as toast } from '@/features/notifications'
import { ContactWriteCards } from './chat-cards'
import { ContactDecisionCards } from './chat-decisions'
import { contactReplies } from './chat-message-model'
import './contact-chat.css'

export function ContactAssistantMessage() {
  const fox = useFoxThread()
  const message = useAuiState(state => state.message.metadata?.custom?.fox as FoxThreadMessage | undefined)
  const lastInRun = useAuiState(state => {
    const current = state.message.metadata?.custom?.fox as FoxThreadMessage | undefined
    return state.thread.messages.findLast(item => item.role === 'assistant'
      && (item.metadata?.custom?.fox as FoxThreadMessage | undefined)?.runId === current?.runId)?.id === state.message.id
  })
  const replies = useMemo(() => message ? contactReplies(message, message.runId ? fox.eventsByRun.get(message.runId) ?? [] : []) : [], [message, fox.eventsByRun])
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
      {replies.map(reply => <section className="fox-contact-message" key={reply.id} data-contact-message-id={reply.id}>
        <div className="fox-contact-bubble"><StreamingText text={reply.text} streaming={false} className="fox-answer-response" /></div>
        <div className="fox-contact-message-meta">
          <FoxCopyMessageButton onCopy={() => copyMessageResponse(reply.text)} />
          <time dateTime={new Date(reply.createdAt).toISOString()}>{formatDateTime(reply.createdAt)}</time>
        </div>
      </section>)}
      {lastInRun && <>
        <RuntimeArtifacts artifacts={artifacts.filter(artifact => artifact.declaredAt != null)} onOpenArtifact={fox.onOpenArtifact} />
        {message.runId && <ContactDecisionCards runId={message.runId} />}
        <ContactWriteCards runId={message.runId} />
        {message.failure && <ErrorPrompt failure={message.failure} />}
        {message.cancelled && <p className="fox-contact-ended" role="status">已停止，发出的消息已保留</p>}
      </>}
    </MessagePrimitive.Root>
  </div>
}
