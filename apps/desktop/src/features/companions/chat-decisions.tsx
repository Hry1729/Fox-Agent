import { Fragment, useContext, useMemo, type ReactNode } from 'react'
import { useAuiState } from '@assistant-ui/react'
import { useFoxThread } from '@/features/chat/thread/fox-thread-context'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { RuntimeApprovalPrompt } from '@/features/chat/components/RuntimeApprovalPrompt'
import { pendingRuntimeApprovals, runtimeQuestionHistory } from '@/features/conversations/model/pending-interactions'
import { ContactQuestionCard } from './chat-question'
import { ContactDecisionExplanation } from './chat-decision'
import { ContactWriteCards, contactWriteCallNeedsCard, WriteCallsContext } from './chat-cards'
import { CompanionTeamCards, useCompanionTeamActions } from './companion-team-cards'

interface ContactTurnEntry {
  id: string
  time: number
  content: ReactNode
}

export function ContactDecisionCards({ runId, replies = [] }: { runId?: string | null; replies?: readonly ContactTurnEntry[] }) {
  const { contactChat } = useFoxThread()
  const detail = contactChat?.detail
  const questions = useMemo(() => detail && runId ? runtimeQuestionHistory(detail).filter(request => request.runId === runId) : [], [detail, runId])
  if (!detail || !runId) return <>{replies.map(reply => <Fragment key={reply.id}>{reply.content}</Fragment>)}</>
  const pending = pendingRuntimeApprovals(detail)
  const activeApproval = pending.toSorted((a, b) => a.requestedAt - b.requestedAt || a.id.localeCompare(b.id))[0]
  const cards = [
    ...detail.approvals.filter(approval => approval.runId === runId).map(approval => ({
      id: approval.id, time: approval.requestedAt,
      explanation: pending.some(item => item.id === approval.id) ? '下一步操作需要你授权。请先查看下面的操作内容与范围。' : '',
      content: <RuntimeApprovalPrompt approval={approval} readOnly={!pending.some(item => item.id === approval.id)} onResolve={contactChat.onResolveApproval ?? (async () => false)} />,
    })),
    ...questions.map(request => ({ id: request.requestId, time: request.createdAt,
      explanation: request.status === 'pending' ? '要继续这件事，我需要你回应下面的问题。' : '',
      content: <ContactQuestionCard request={request} onSubmit={contactChat.onSubmitQuestion} disabled={Boolean(activeApproval)} />,
    })),
  ].sort((a, b) => a.time - b.time || a.id.localeCompare(b.id))
  const entries = [
    ...replies.map(reply => ({ ...reply, kind: 'reply' as const })),
    ...cards.map(card => ({ ...card, kind: 'card' as const })),
  ].sort((left, right) => left.time - right.time)
  return <>{entries.map((entry, index) => <Fragment key={entry.id}>
    {entry.kind === 'card' ? <>
      {entry.explanation && entries[index - 1]?.kind !== 'reply' && <ContactDecisionExplanation>{entry.explanation}</ContactDecisionExplanation>}
      <div className="fox-contact-decision-entry">{entry.content}</div>
    </> : entry.content}
  </Fragment>)}<CompanionTeamCards runId={runId} /></>
}

export function ContactOrphanDecisions() {
  const { contactChat } = useFoxThread()
  const writes = useContext(WriteCallsContext)
  const teamActions = useCompanionTeamActions()
  const assistantRuns = useAuiState(state => state.thread.messages.flatMap(message => {
    const fox = message.metadata?.custom?.fox as FoxThreadMessage | undefined
    return message.role === 'assistant' && fox?.runId ? [fox.runId] : []
  }).join('\n'))
  if (!contactChat) return null
  const visible = new Set(assistantRuns.split('\n'))
  const { detail } = contactChat
  const loadedRuns = new Set(detail.runs?.map(run => run.id))
  if (detail.lastRun) loadedRuns.add(detail.lastRun.id)
  const oldestMessageAt = detail.messages.reduce((oldest, message) => Math.min(oldest, message.createdAt), Infinity)
  const runs = new Set([
    ...detail.approvals.map(approval => approval.runId),
    ...detail.runtimeEvents.filter(event => event.eventType === 'user.question.requested').map(event => event.runId),
    ...writes.filter(contactWriteCallNeedsCard).map(call => call.runId),
    ...(teamActions ?? []).filter(action => action.status === 'pending' || loadedRuns.has(action.runId)
      || !detail.hasEarlierMessages || action.createdAt >= oldestMessageAt).map(action => action.runId),
  ])
  return <>{[...runs].filter(id => !visible.has(id)).map(id => <div className="fox-thread-msg is-assistant fox-contact-run" key={id}>
    <ContactDecisionCards runId={id} /><ContactWriteCards runId={id} />
  </div>)}</>
}
