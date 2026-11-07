import { SharedDecision } from './shared-decision'
import { createContext, useContext, useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { Button } from '@/components/ui/button'
import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { ToolCallRecord } from '@/features/conversations/model/types'
import { contacts, direct, type CompanionNotes, type Companion } from './api'
import { showContactError, useContacts } from './context'
import { ContactChoice, ContactDecision } from './chat-decision'

export const WriteCallsContext = createContext<ToolCallRecord[]>([])
const labels: Record<string, string> = { created: '已记住', existing: '之前已经记过', updated: '已更新', removed: '已删除', conflict: '这条记忆与已有内容冲突', pending: '需要确认', refused: '未写入', applied: '已整理到专题' }

export function contactWriteCallNeedsCard(call: ToolCallRecord) {
  const value = call.result as { status?: string; memoryId?: string; details?: { status?: string; memoryId?: string } } | null
  const result = value?.status ? value : value?.details
  return Boolean(result && (result.status === 'created' && result.memoryId || ['pending', 'conflict', 'refused'].includes(result.status ?? '')))
}

export function ContactWriteCards({ runId }: { runId: string | null }) {
  const calls = useContext(WriteCallsContext)
  const contact = useContacts()?.current
  if (!contact || !runId) return null
  return <>{calls.filter(c => c.runId === runId && contactWriteCallNeedsCard(c)).map(call => <WriteResult key={call.id} contact={contact} call={call} />)}</>
}

function WriteResult({ contact, call }: { contact: Companion; call: ToolCallRecord }) {
  const [busy, setBusy] = useState(false)
  const [records, setRecords] = useState<CompanionNotes | null>(null)
  const [error, setError] = useState('')
  const value = call.result as { status?: string; details?: { status?: string }; memoryId?: string; moveId?: string; requestId?: string; requests?: Array<{ requestId: string }>; code?: string; overBy?: number } | null
  const result = value?.status ? value : value?.details as typeof value
  const status = result?.status
  const requestKey = [result?.requestId, ...(result?.requests ?? []).map(q => q.requestId)].filter(Boolean).join(',')
  const pending = Boolean(requestKey)
  useEffect(() => {
    if (!pending && status !== 'created') return
    let live = true
    contacts.notes(contact.id).then(next => { if (live) setRecords(next) }).catch(e => { if (live) setError(desktopErrorDetails(e).message) })
    return () => { live = false }
  }, [contact.id, pending, requestKey, status])
  if (!status || !result) return null
  const receipt = records?.writeReceipts?.find(item => item.memoryId === result.memoryId && item.runId === call.runId)
  const questions = (records?.requestHistory ?? records?.requests.map(request => ({ ...request, status: 'open' })) ?? [])
    .filter(request => requestKey.split(',').includes(request.id))
  const shared = (records?.sharedRequestHistory ?? records?.sharedRequests.map(request => ({ ...request, status: 'open' })) ?? [])
    .find(request => request.id === result.requestId)
  const input = call.input as { content?: string } | null
  const decide = async (name: string, data: Record<string, unknown>) => {
    setBusy(true)
    setError('')
    try {
      const response = await direct<{ status?: string; overBy?: number }>(name, { companionId: contact.id, ...data })
      setRecords(await contacts.notes(contact.id))
      if (response?.status === 'refused') setError(response.overBy ? `超过主笔记上限 ${response.overBy} 字，请先整理笔记。` : '此次操作未执行，请检查笔记后重试。')
    } catch (e) { setError(desktopErrorDetails(e).message) } finally { setBusy(false) }
  }
  return <div className="fox-contact-write-results">
    {!pending && <ContactDecision title={receipt?.status === 'undone' ? '已撤销' : labels[status] || '笔记已更新'}>
      {input?.content && <p>{input.content}</p>}
      {status === 'refused' && <p>{result.overBy ? '超过主笔记上限 ' + result.overBy + ' 字' : result.code}</p>}
      {receipt?.status === 'created' && result.memoryId && <Button variant="ghost" size="sm" disabled={busy} onClick={() => void decide('companion_memory_undo', { runId: call.runId, memoryId: result.memoryId })}>撤销</Button>}
    </ContactDecision>}
    {questions.map(question => {
      const action = question.action === 'move' ? '移动' : question.action === 'remove' ? '删除' : '改写'
      const editable = question.status === 'open'
      return <ContactDecision key={question.id} title={`可以${action}这条笔记吗？`}>
        <p>{question.content}</p>
        {question.action === 'patch' && <p>改为：{question.payload?.content}</p>}
        {question.action === 'move' && <p>目标专题：{question.payload?.topic}</p>}
        <div className="fox-contact-choices">{[true, false].map((approve, index) => <ContactChoice key={String(approve)} index={index}
          selected={question.status === (approve ? 'approved' : 'rejected')} disabled={busy || !editable}
          onClick={() => void decide('companion_note_request_resolve', { requestId: question.id, approve })}>
          {approve ? `确认${action}` : `暂不${action}`}
        </ContactChoice>)}</div>
        {!editable && <p className="fox-contact-receipt" role="status">{question.status === 'approved' ? `已确认${action}` : question.status === 'rejected' ? `已选择暂不${action}` : '此确认已过期，请查看最新笔记'}</p>}
      </ContactDecision>
    })}
    {shared && <ContactDecision title="要记住这个偏好吗？">
      {shared.status === 'open' ? <SharedDecision chat request={shared} busy={busy} onChoose={(decision, replaceMemoryId) => void decide('companion_shared_request_resolve', { requestId: shared.id, decision, replaceMemoryId })} />
        : <><p>{shared.content}</p><p className="fox-contact-receipt">{({ added: '已选择记住这条', replaced: '已选择替换已有记忆', rejected: '已选择暂不记住', stale: '此确认已过期，请查看最新笔记' } as Record<string, string>)[shared.status]}</p></>}
    </ContactDecision>}
    {pending && !records && !error && <p role="status">正在读取确认记录…</p>}
    {records && pending && !questions.length && !shared && <p className="fox-contact-receipt">此确认已处理</p>}
    {error && <p role="alert" className="fox-contact-decision-error">{error}</p>}
  </div>
}

export function ImportSuggestion({ contact }: { contact: Companion }) {
  const [paths, setPaths] = useState<string[]>([])
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    let active = true
    const listener = listen<{ companionId: string; path: string }>('fox://companion-import-suggested', ({ payload }) => {
      if (active && payload.companionId === contact.id) setPaths(items => items.includes(payload.path) ? items : [...items, payload.path])
    })
    return () => { active = false; void listener.then(stop => stop()).catch(() => { }) }
  }, [contact.id])
  return <>{paths.map(path => <aside className="fox-contact-banner" key={path}><p>把 {path} 复制到{contact.displayName}的工作目录？</p><Button disabled={busy} size="sm" onClick={() => { setBusy(true); void contacts.importFiles(contact.id, [path]).then(() => setPaths(items => items.filter(p => p !== path))).catch(showContactError).finally(() => setBusy(false)) }}>加入工作目录</Button><Button size="sm" variant="ghost" onClick={() => setPaths(items => items.filter(p => p !== path))}>忽略</Button></aside>)}</>
}

export function ProjectGrantPrompt({ contact, projectId, conversationId }: { contact: Companion; projectId: string; conversationId: string }) {
  const key = 'fox.contactGrantNotice.' + conversationId + '.' + contact.id
  const [offer, setOffer] = useState(false)
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    let live = true
    if (!sessionStorage.getItem(key)) void contacts.grants(contact.id).then(items => { if (live) setOffer(!items.some(g => g.projectId === projectId)) }).catch(showContactError)
    return () => { live = false }
  }, [key, contact.id, projectId])
  if (!offer) return null
  const dismiss = () => { sessionStorage.setItem(key, 'dismissed'); setOffer(false) }
  return <aside className="fox-contact-banner"><p>允许此项目中已引入该专家的对话读取{contact.displayName}的笔记？</p><Button size="sm" disabled={busy} onClick={() => { setBusy(true); void contacts.setGrant(contact.id, projectId, true).then(dismiss).catch(showContactError).finally(() => setBusy(false)) }}>授权</Button><Button size="sm" variant="ghost" onClick={dismiss}>暂不</Button></aside>
}
