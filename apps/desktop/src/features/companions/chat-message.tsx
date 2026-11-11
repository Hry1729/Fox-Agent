import { Fragment, useMemo } from 'react'
import { MessagePrimitive, useAuiState } from '@assistant-ui/react'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { useFoxThread } from '@/features/chat/thread/fox-thread-context'
import { RuntimeArtifacts } from '@/features/chat/runtime-artifacts'
import { resolveFinalArtifactReference } from '@/features/chat/final-artifact-references'
import { ErrorPrompt } from '@/features/chat/demo/fox-demo-prompts'
import { notify as toast } from '@/features/notifications'
import { ContactWriteCards } from './chat-cards'
import { ContactDecisionCards } from './chat-decisions'
import { visibleContactReplies } from './chat-message-model'
import { ContactTimeDivider } from './chat-time'
import { ContactReplyBubble } from './marks'
import { useContacts } from './context'
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
    message.runId ? fox.contactKernelPreviewsByRun?.get(message.runId) : undefined,
  ) : [], [message, fox.eventsByRun, fox.contactBubblesByRun, fox.contactKernelPreviewsByRun])
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
      {/* SPEC item 9：不再渲染每条回复旁的模型头像——顶部胶囊已经有头像。 */}
      <div className="fox-contact-run-body">
        {replies.map(reply => <Fragment key={reply.id}><ContactTimeDivider messageId={reply.id} /><ContactReplyBubble contact={contact} reply={reply}
          initialMark={fox.contactBubbleMarks?.get(reply.id)} /></Fragment>)}
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
