import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { contacts, type Companion } from './api'
import { useContacts, showContactError } from './context'
import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'

export function ContactDelete({ contact, close }: { contact: Companion; close: () => void }) {
  const ctx = useContacts()
  const [preview, setPreview] = useState<Awaited<ReturnType<typeof contacts.preview>> | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [revision, setRevision] = useState(0)
  useEffect(() => {
    let current = true
    setPreview(null); setError('')
    contacts.preview(contact.id).then(value => { if (current) setPreview(value) }).catch(cause => { if (current) setError(desktopErrorDetails(cause).message) })
    return () => { current = false }
  }, [contact.id, revision])
  const remove = async () => {
    if (!preview) return
    setBusy(true)
    try { await contacts.delete(contact.id, preview.planHash); ctx?.leave(contact); await ctx?.refresh(); close() }
    catch (cause) { setPreview(null); setError(desktopErrorDetails(cause).message); void ctx?.refresh().catch(showContactError) }
    finally { setBusy(false) }
  }
  const plan = preview?.plan
  return <Dialog open onOpenChange={next => { if (!next && !busy) close() }}><DialogContent>
    <DialogHeader><DialogTitle>删除{contact.displayName}？</DialogTitle><DialogDescription>删除后无法撤销。以后再添加同一专家，会创建新的联系人。</DialogDescription></DialogHeader>
    {error ? <p role="alert">{error} <Button variant="ghost" onClick={() => setRevision(r => r + 1)}>重新预览</Button></p> : !plan ? <p role="status">正在核对删除清单…</p> : <div className="fox-contact-section">
      <p>将删除 {plan.messages} 条消息、{plan.notes} 条主笔记、{plan.topics} 个专题，以及这段私聊的模型上下文。</p>
      <p>将撤销 {plan.grantedProjects.length} 个项目授权{plan.grantedProjects.length ? '（' + plan.grantedProjects.join('、') + '）' : ''}。</p>
      <p>将清理 {plan.files.filter(file => file.disposition === 'remove').length} 项附件副本和内部文件。</p>
      <p>保留 {plan.sharedMemories} 条共享记忆，以及工作目录里的 {plan.workspaceFiles} 个文件。</p>
      <code>{plan.workspacePath}</code>
      {plan.files.length > 0 && <details><summary>文件清单（{plan.files.length}）</summary>{plan.files.map(file => <p key={file.path}>{file.disposition === 'retain' ? '保留' : '移除'} · {file.path}</p>)}</details>}
    </div>}
    <DialogFooter><Button variant="outline" disabled={busy} onClick={close}>取消</Button><Button variant="destructive" disabled={!preview || busy} onClick={() => void remove()}>永久删除</Button></DialogFooter>
  </DialogContent></Dialog>
}
