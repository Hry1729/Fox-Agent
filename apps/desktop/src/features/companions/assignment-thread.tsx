// Adapted from OpenMausBot src/components/HandoffReply.tsx and ThreadChip.tsx at 1a4e4da9f380804ad98c7fd1b141bb0dbfafced4 (Apache-2.0).
import { useCallback, useEffect, useId, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { ArrowLeftRight, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useUserProfile } from '@/features/profile/user-profile'
import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { CompanionAssignment, CompanionCoordinationThread } from '@/features/conversations/model/types'
import { COMPANION_ASSIGNMENTS_CHANGED, contacts } from './api'
import { ContactDecision } from './chat-decision'
import { ContactAvatar } from './chat-controls'
import { useContacts } from './context'
import './assignment-thread.css'

export function CoordinationTopbar({ thread }: { thread: CompanionCoordinationThread }) {
  const context = useContacts()
  const { profile } = useUserProfile(context?.userProfile)
  const recipient = context?.list.companions.find(contact => contact.id === thread.recipientCompanionId)
  const source = thread.sourceCompanionId ? context?.list.companions.find(contact => contact.id === thread.sourceCompanionId) : { displayName: profile.name, avatarRef: profile.avatar }
  return <header className="fox-coordination-topbar" aria-label="伙伴间的对话">
    <div className="fox-coordination-heading">
      <ContactAvatar contact={source ?? { displayName: thread.sourceName, avatarRef: '' }} /><strong>{source?.displayName ?? thread.sourceName}</strong>
      <ArrowLeftRight size={14} aria-label="消息往来" />
      <ContactAvatar contact={recipient ?? { displayName: '伙伴', avatarRef: '' }} /><strong>{recipient?.displayName ?? '接收方伙伴'}</strong>
    </div>
  </header>
}

export function CoordinationClose({ thread }: { thread: CompanionCoordinationThread }) {
  const context = useContacts()
  const recipient = context?.list.companions.find(contact => contact.id === thread.recipientCompanionId)
  const destination = thread.sourceConversationId ?? recipient?.conversationId
  return <div className="fox-coordination-close"><Button type="button" variant="ghost" disabled={!destination && !context?.closeCoordination} aria-label="关闭伙伴对话"
    onClick={() => context?.closeCoordination ? context.closeCoordination() : destination && context?.openCoordination?.(destination, thread.sourceConversationId && !thread.sourceCompanionId ? 'chat' : 'companions')}>
    <X size={15} aria-hidden />关闭聊天
  </Button></div>
}

export function useCompanionAssignments(conversationId: string | null) {
  const [records, setRecords] = useState<CompanionAssignment[]>([])
  const [error, setError] = useState('')
  const revision = useRef(0)
  const reload = useCallback(async () => {
    if (!conversationId) return
    const ticket = ++revision.current
    try {
      const next = await contacts.assignments(conversationId)
      if (ticket === revision.current) {
        setRecords(next)
        setError('')
      }
    } catch (cause) {
      if (ticket === revision.current) setError(desktopErrorDetails(cause).message)
    }
  }, [conversationId])

  useEffect(() => {
    setRecords([])
    setError('')
    if (!conversationId) return
    let active = true
    const subscription = listen<{ conversationIds: string[] }>(COMPANION_ASSIGNMENTS_CHANGED, ({ payload }) => {
      if (active && payload.conversationIds.includes(conversationId)) void reload()
    })
    void subscription.catch(() => {}).then(() => { if (active) void reload() })
    return () => {
      active = false
      ++revision.current
      void subscription.then(stop => stop()).catch(() => {})
    }
  }, [conversationId, reload])

  const answer = async (id: string, text: string) => {
    const updated = await contacts.answerAssignment(id, text)
    setRecords(current => current.map(record => record.id === id ? updated : record))
    void reload()
  }
  const cancel = async (id: string) => {
    const updated = await contacts.cancelAssignment(id)
    setRecords(current => current.map(record => record.id === id ? updated : record))
    void reload()
  }
  return { records, error, reload, answer, cancel }
}

export function AssignmentActions({ assignment, onAnswer, onCancel, cancelOnly = false }: {
  assignment: CompanionAssignment
  onAnswer: (id: string, text: string) => Promise<void>
  onCancel: (id: string) => Promise<void>
  cancelOnly?: boolean
}) {
  const [busy, setBusy] = useState(false)
  const perform = async (operation: () => Promise<void>) => {
    setBusy(true)
    try { await operation() }
    catch (cause) { console.error('Companion assignment answer failed', cause) }
    finally { setBusy(false) }
  }
  const needsAnswer = !cancelOnly && assignment.status === 'needs_input' && assignment.question != null
  const cancellable = (assignment.status === 'queued' && !assignment.recipientRunId)
    || (assignment.status === 'needs_input' && assignment.question != null)
  return <>
    {cancelOnly && assignment.status === 'cancelled' && <p role="status" className="fox-assignment-status">已取消</p>}
    {needsAnswer && assignment.question && <AssignmentQuestion assignmentId={assignment.id} question={assignment.question} busy={busy}
      onAnswer={(id, text) => perform(() => onAnswer(id, text))} />}
    {cancelOnly && cancellable && <div className="fox-assignment-actions"><Button type="button" size="sm" variant="ghost" disabled={busy}
      onClick={() => void perform(() => onCancel(assignment.id))}>{busy ? '正在取消…' : '取消协作'}</Button></div>}
  </>
}

function AssignmentQuestion({ assignmentId, question, busy, onAnswer }: {
  assignmentId: string
  question: NonNullable<CompanionAssignment['question']>
  busy: boolean
  onAnswer: (id: string, text: string) => Promise<void>
}) {
  const answerId = useId()
  const [draft, setDraft] = useState('')
  const submit = (answer: string) => onAnswer(assignmentId, answer)
  return <ContactDecision title="需要你补充信息" description={question.prompt}>
    <form className="fox-assignment-question" onSubmit={event => { event.preventDefault(); if (draft.trim()) void submit(draft.trim()) }}>
      <label htmlFor={answerId}>你的回答</label>
      <textarea id={answerId} className="fox-contact-operation" rows={2} maxLength={4000} value={draft} onChange={event => setDraft(event.currentTarget.value)} disabled={busy} />
      {question.defaultAnswer && <p className="fox-contact-decision-note">默认回答 {question.defaultAnswer}</p>}
      <div>
        <Button size="sm" type="submit" disabled={busy || !draft.trim()}>{busy ? '正在提交…' : '提交回答'}</Button>
        {question.defaultAnswer && <Button size="sm" type="button" variant="outline" disabled={busy} onClick={() => void submit(question.defaultAnswer!)}>采用默认回答</Button>}
      </div>
    </form>
  </ContactDecision>
}
