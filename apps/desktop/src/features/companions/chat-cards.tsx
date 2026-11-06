import { SharedDecision } from './shared-decision'
import { createContext, useContext, useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { Button } from '@/components/ui/button'
import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { ToolCallRecord } from '@/features/conversations/model/types'
import { contacts, direct, type CompanionNotes, type Companion } from './api'
import { showContactError, useContacts } from './context'

export const WriteCallsContext = createContext<ToolCallRecord[]>([])
const labels: Record<string, string> = { created: '已记住', existing: '之前已经记过', updated: '已更新', removed: '已删除', conflict: '这条记忆与已有内容冲突', pending: '需要确认', refused: '未写入', applied: '已整理到专题' }

export function ContactWriteCards({ runId }: { runId: string | null }) {
  const calls = useContext(WriteCallsContext)
  const contact = useContacts()?.current
  if (!contact || !runId) return null
  return <>{calls.filter(c => c.runId === runId).map(call => <WriteResult key={call.id} contact={contact} call={call} />)}</>
}

function WriteResult({ contact, call }: { contact: Companion; call: ToolCallRecord }) {
  const [done, setDone] = useState('')
  const [busy, setBusy] = useState(false)
  const [records, setRecords] = useState<CompanionNotes | null>(null)
  const [error, setError] = useState('')
  const value = call.result as { status?: string; details?: { status?: string }; memoryId?: string; moveId?: string; requestId?: string; requests?: Array<{ requestId: string }>; code?: string; overBy?: number } | null
  const result = value?.status ? value : value?.details as typeof value
  const status = result?.status
  const requestKey = [result?.requestId, ...(result?.requests ?? []).map(q => q.requestId)].filter(Boolean).join(',')
  const pending = Boolean(requestKey)
  useEffect(() => {
    if (!pending) return
    let live = true
    contacts.notes(contact.id).then(next => { if (live) setRecords(next) }).catch(e => { if (live) setError(desktopErrorDetails(e).message) })
    return () => { live = false }
  }, [contact.id, pending, requestKey])
  if (!status || !result) return null
  const questions = records?.requests.filter(r => requestKey.split(',').includes(r.id)) ?? []
  const shared = records?.sharedRequests.find(r => r.id === result.requestId)
  const input = call.input as { content?: string } | null
  const decide = async (name: string, data: Record<string, unknown>, label: string) => {
    setBusy(true)
    try {
      const response = await direct<{ status?: string }>(name, { companionId: contact.id, ...data })
      if (pending && response?.status !== 'stale') setRecords(await contacts.notes(contact.id))
      if (questions.length <= 1 || response?.status === 'stale') setDone(response?.status === 'stale' ? '此确认已过期，请查看最新笔记' : label)
    } catch (e) { setError(desktopErrorDetails(e).message) } finally { setBusy(false) }
  }
  return <aside className="fox-contact-section fox-contact-entry"><strong>{done || labels[status] || status}</strong>{input?.content && <p>{input.content}</p>}
    {status === 'refused' && <p>{result.overBy ? '超过主笔记上限 ' + result.overBy + ' 字' : result.code}</p>}
    {!done && status === 'created' && result.memoryId && <Button variant="ghost" size="sm" disabled={busy} onClick={() => void decide('companion_memory_undo', { runId: call.runId, memoryId: result.memoryId }, '已撤销')}>撤销</Button>}
    {!done && questions.map(q => <div key={q.id}><strong>{q.action === 'move' ? '确认移入专题' : q.action === 'remove' ? '确认删除笔记' : '确认改写笔记'}</strong><p>{q.content}</p>{q.action === 'patch' && <p>改为：{q.payload?.content}</p>}{q.action === 'move' && <p>目标专题：{q.payload?.topic}</p>}{[true, false].map(approve => <Button size="sm" variant="outline" key={String(approve)} disabled={busy} onClick={() => void decide('companion_note_request_resolve', { requestId: q.id, approve }, approve ? '已同意' : '已拒绝')}>{approve ? '同意' : '拒绝'}</Button>)}</div>)}
    {!done && shared && <SharedDecision request={shared} busy={busy} onChoose={(decision, replaceMemoryId) => void decide('companion_shared_request_resolve', { requestId: shared.id, decision, replaceMemoryId }, decision === 'replace' ? '已替换' : decision === 'add' ? '已保存' : '已拒绝')} />}
    {records && pending && !questions.length && !shared && !done && <small>已处理</small>}{error && <p role="alert">{error}</p>}</aside>
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
