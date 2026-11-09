import { useCallback, useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { contacts, type Companion, type CompanionFileEntry, type CompanionFiles } from './api'
import { formatSearchTime } from './chat-search'
import { showContactError, useContacts } from './context'
import './contacts.css'

export const FILES_WORKDIR_HINT = '手工放入工作目录、没有消息记录的文件，请用「打开工作目录」查看。'

function FileRow({ contact, entry }: { contact: Companion; entry: CompanionFileEntry }) {
  const ctx = useContacts()
  const [busy, setBusy] = useState(false)
  const act = async (action: () => Promise<unknown>) => {
    setBusy(true)
    try { await action() } catch (e) { showContactError(e) } finally { setBusy(false) }
  }
  return <div className="fox-contact-file" aria-label={entry.name}>
    <div className="fox-contact-file-head">
      <b>{entry.name}</b>
      <time dateTime={new Date(entry.updatedAt).toISOString()}>{formatSearchTime(entry.updatedAt)}</time>
    </div>
    <p className="fox-contact-file-source">
      {entry.origin === 'task' && <>定时任务「{entry.taskName ?? '未命名'}」 · </>}
      {entry.source ? `来源消息 · ${formatSearchTime(entry.source.at)}` : '暂无来源消息'}
      {entry.missing && <span className="fox-contact-file-missing" role="status"> · 文件缺失</span>}
    </p>
    <div className="fox-contact-actions">
      <Button size="sm" variant="outline" disabled={busy || entry.missing} onClick={() => void act(() => contacts.openFile(contact.id, entry.kind, entry.id))}>打开</Button>
      <Button size="sm" variant="outline" disabled={busy || entry.missing} onClick={() => void act(() => contacts.saveFile(contact.id, entry.kind, entry.id))}>另存为</Button>
      <Button size="sm" variant="ghost" disabled={busy || !entry.source} onClick={() => entry.source && ctx?.jumpToMessage?.(entry.source.bubbleId)}>回到消息</Button>
    </div>
  </div>
}

export function ContactFiles({ contact, onBack }: { contact: Companion; onBack?: () => void }) {
  const [data, setData] = useState<CompanionFiles | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const reload = useCallback(async () => {
    try { setData(await contacts.files(contact.id)); setError('') } catch (e) { setError(desktopErrorDetails(e).message) }
  }, [contact.id])
  useEffect(() => { void reload() }, [reload])
  const openWorkspace = async () => {
    setBusy(true)
    try { await desktopClient.openProjectFolder(contact.conversationId) } catch (e) { showContactError(e) } finally { setBusy(false) }
  }
  return <div className="fox-contact-files" aria-busy={busy}>
    <div className="fox-contact-capabilities-head"><h3>文件与成果</h3>
      {onBack && <Button size="sm" variant="ghost" onClick={onBack}>返回资料</Button>}</div>
    {error && <p role="alert">{error} <button type="button" onClick={() => void reload()}>重试</button></p>}
    {!data ? <p>正在读取文件与成果…</p> : <>
      <section className="fox-contact-section"><h3>我提供的文件</h3>
        {data.provided.length ? data.provided.map(entry => <FileRow key={entry.id} contact={contact} entry={entry} />)
          : <p className="fox-contact-capability-hint">还没有通过消息提供过文件。</p>}
      </section>
      <section className="fox-contact-section"><h3>联系人生成的结果</h3>
        {data.generated.length ? data.generated.map(entry => <FileRow key={entry.id} contact={contact} entry={entry} />)
          : <p className="fox-contact-capability-hint">还没有生成过文件。</p>}
      </section>
      <p className="fox-contact-capability-hint">{FILES_WORKDIR_HINT}</p>
      <div className="fox-contact-actions"><Button size="sm" variant="outline" disabled={busy} onClick={() => void openWorkspace()}>打开工作目录</Button></div>
    </>}
  </div>
}