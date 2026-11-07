import { useContext, useMemo } from 'react'
import { useAuiState } from '@assistant-ui/react'
import { useFoxThread } from '@/features/chat/thread/fox-thread-context'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { RuntimeApprovalPrompt } from '@/features/chat/components/RuntimeApprovalPrompt'
import { pendingRuntimeApprovals, runtimeQuestionHistory } from '@/features/conversations/model/pending-interactions'
import { ContactQuestionCard } from './chat-question'
import { ContactWriteCards, contactWriteCallNeedsCard, WriteCallsContext } from './chat-cards'

export function ContactDecisionCards({ runId }: { runId: string }) {
  const { contactChat } = useFoxThread()
  const detail = contactChat?.detail
  const questions = useMemo(() => detail ? runtimeQuestionHistory(detail).filter(request => request.runId === runId) : [], [detail, runId])
  if (!detail) return null
  const pending = pendingRuntimeApprovals(detail)
  const activeApproval = pending.toSorted((a, b) => a.requestedAt - b.requestedAt || a.id.localeCompare(b.id))[0]
  const cards = [
    ...detail.approvals.filter(approval => approval.runId === runId).map(approval => ({
      id: approval.id, time: approval.requestedAt,
      content: <RuntimeApprovalPrompt approval={approval} readOnly={!pending.some(item => item.id === approval.id)} onResolve={contactChat.onResolveApproval ?? (async () => false)} />,
    })),
    ...questions.map(request => ({ id: request.requestId, time: request.createdAt,
      content: <ContactQuestionCard request={request} onSubmit={contactChat.onSubmitQuestion} disabled={Boolean(activeApproval)} />,
    })),
  ].sort((a, b) => a.time - b.time || a.id.localeCompare(b.id))
  return <>{cards.map(card => <div key={card.id} className="fox-contact-decision-entry">{card.content}</div>)}</>
}

export function ContactOrphanDecisions() {
  const { contactChat } = useFoxThread()
  const writes = useContext(WriteCallsContext)
  const assistantRuns = useAuiState(state => state.thread.messages.flatMap(message => {
    const fox = message.metadata?.custom?.fox as FoxThreadMessage | undefined
    return message.role === 'assistant' && fox?.runId ? [fox.runId] : []
  }).join('\n'))
  if (!contactChat) return null
  const visible = new Set(assistantRuns.split('\n'))
  const runs = new Set([
    ...contactChat.detail.approvals.map(approval => approval.runId),
    ...contactChat.detail.runtimeEvents.filter(event => event.eventType === 'user.question.requested').map(event => event.runId),
    ...writes.filter(contactWriteCallNeedsCard).map(call => call.runId),
  ])
  return <>{[...runs].filter(id => !visible.has(id)).map(id => <div className="fox-thread-msg is-assistant fox-contact-run" key={id}>
    <ContactDecisionCards runId={id} /><ContactWriteCards runId={id} />
  </div>)}</>
}
