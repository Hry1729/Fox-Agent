import { SharedDecision } from './shared-decision'
import { useCallback, useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { contacts, direct, type Companion, type CompanionNotes, type NoteTopic } from './api'
import { showContactError, useContacts } from './context'

type TopicVersion = NoteTopic & { content: string }
type MutationResult = { status?: string; overBy?: number; current?: TopicVersion }
type TopicEdit = Pick<NoteTopic, 'id' | 'title' | 'contentSha256'> & { content: string; aliases: string }
const emptyTopic: TopicEdit = { id: '', title: '', contentSha256: '', content: '', aliases: '' }

export function ContactNotes({ contact }: { contact: Companion }) {
  const companionId = contact.id
  const ctx = useContacts()
  const [data, setData] = useState<CompanionNotes | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [content, setContent] = useState('')
  const [editing, setEditing] = useState<{ id: string; version: number } | null>(null)
  const [topic, setTopic] = useState<TopicEdit | null>(null)
  const [conflict, setConflict] = useState<TopicVersion | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [confirm, setConfirm] = useState<{ label: string; command: string; args: Record<string, unknown> } | null>(null)
  const reload = useCallback(async () => { setData(await contacts.notes(companionId)) }, [companionId])
  useEffect(() => { void reload().catch(e => setError(desktopErrorDetails(e).message)) }, [reload])
  const act = async (name: string, args: Record<string, unknown> = {}, onSaved?: () => void) => {
    setBusy(true); setError('')
    try {
      const value = await direct<MutationResult | null>(name, { companionId, ...args })
      await reload()
      if (value?.status === 'refused') setError(value.overBy ? '超过主笔记上限 ' + value.overBy + ' 字。请缩短笔记或转入专题。' : '此操作未获准。')
      else if (value?.status === 'conflict' && value.current) {
        setConflict(value.current)
        setConfirm(null)
        setError('专题已在别处修改。请对照当前版本，决定保留哪一份。')
      } else if (value?.status === 'stale') setError('这项请求已过期，请打开最新笔记后再操作。')
      else { setConflict(null); onSaved?.() }
    } catch (e) { setError(desktopErrorDetails(e).message) } finally { setBusy(false) }
  }
  const openTopic = async (id: string) => {
    try {
      const record = await direct<NoteTopic & { content: string }>('companion_topic_read', { companionId, topicId: id })
      setConflict(null); setTopic({ ...record, aliases: record.aliases.join('，') })
    } catch (e) { showContactError(e) }
  }
  const count = (text: string) => Array.from(text.replace(/\r\n/g, '\n').trim()).length
  const original = data?.notes.find(note => note.id === editing?.id)
  const projected = (data?.usedChars ?? 0) + (original ? original.state === 'confirmed' && original.enabled ? count(content) - count(original.content) : 0 : count(content))
  return <div aria-busy={busy}>
    {error && <p role="alert">{error} <button type="button" onClick={() => void reload().then(() => setError('')).catch(showContactError)}>重新加载</button></p>}
    {!data ? <p>正在读取笔记…</p> : <>
      <section className="fox-contact-section"><h3>主笔记 · {data.usedChars} / {data.limit} 字</h3>
        {data.notes.filter(n => !n.deletedAt).map(n => <div className="fox-contact-entry" key={n.id}><p>{n.content}</p><small>{n.createdBy === 'user' ? '用户写入' : '联系人写入'} · {n.state === 'candidate' ? '待确认' : n.enabled ? '启用' : '停用'}</small>
          <div className="fox-contact-actions"><Button size="sm" variant="ghost" onClick={() => { setContent(n.content); setEditing({ id: n.id, version: n.version }) }}>编辑</Button>
            {n.openConflictId ? <><button disabled={busy} onClick={() => void act('companion_note_conflict_resolve', { conflictId: n.openConflictId, accept: true })}>采用候选</button><button disabled={busy} onClick={() => void act('companion_note_conflict_resolve', { conflictId: n.openConflictId, accept: false })}>保留旧内容</button></> :
              n.state === 'candidate' ? <button disabled={busy} onClick={() => void act('companion_note_confirm', { memoryId: n.id, expectedVersion: n.version })}>确认</button> :
                <button disabled={busy} onClick={() => void act('companion_note_set_enabled', { memoryId: n.id, expectedVersion: n.version, enabled: !n.enabled })}>{n.enabled ? '停用' : '启用'}</button>}
            <button disabled={busy} onClick={() => setConfirm({ label: '删除这条笔记？', command: 'companion_note_delete', args: { memoryId: n.id, expectedVersion: n.version } })}>删除</button></div></div>)}
        <form className="fox-contact-section" onSubmit={e => { e.preventDefault(); void act('companion_note_save_user', { content, memoryId: editing?.id, expectedVersion: editing?.version }, () => { setEditing(null); setContent('') }) }}>
          <label>{editing ? '编辑主笔记' : '新主笔记'}<textarea value={content} onChange={e => setContent(e.target.value)} required /></label>
          <output aria-live="polite">保存后 {projected.toLocaleString('en-US')} / 4,000 字{projected > data.limit ? '，还需腾出 '+(projected - data.limit)+' 字' : ''}</output>
          <div className="fox-contact-actions"><Button size="sm" disabled={busy || !content.trim()}>保存笔记</Button>{editing && <Button type="button" size="sm" variant="ghost" onClick={() => { setEditing(null); setContent('') }}>取消编辑</Button>}
            <Button type="button" size="sm" variant="ghost" disabled={!content || busy} onClick={() => void act('companion_note_draft_save', { content })}>存为草稿</Button></div></form>
        {data.draft && <div className="fox-contact-entry"><h4>草稿</h4><p>{data.draft}</p><button type="button" onClick={() => { setContent(data.draft ?? ''); setEditing(null) }}>继续编辑</button> · <button type="button" onClick={() => { setTopic({ ...emptyTopic, content: data.draft ?? '' }); setExpanded(true) }}>转存为专题</button> · <button type="button" onClick={ctx?.askToOrganize}>请{contact.displayName}帮我整理</button></div>}
        {data.notes.length > 0 && <button type="button" disabled={busy} onClick={() => setConfirm({ label: '清空联系人的主笔记？', command: 'companion_note_clear', args: {} })}>清空主笔记…</button>}
      </section>
      <section className="fox-contact-section"><h3>专题</h3>
        {data.topics.map(t => <button type="button" key={t.id} onClick={() => { setExpanded(false); void openTopic(t.id) }}>{t.title}</button>)}
        <Button type="button" variant="outline" size="sm" onClick={() => { setTopic({ ...emptyTopic }); setExpanded(false) }}>新建专题</Button>
        {topic && <form className="fox-contact-section" onSubmit={e => {
e.preventDefault(); const args = { title: topic.title, aliases: topic.aliases.split(/[，,]/).map(v => v.trim()).filter(Boolean) }
          void act(expanded ? 'companion_note_draft_to_topic' : 'companion_topic_save', expanded ? args : { ...args, topicId: topic.id || undefined, expectedSha: topic.contentSha256 || undefined, content: topic.content }, () => setTopic(null))
}}>
          <label>专题标题<input required maxLength={60} value={topic.title} onChange={e => setTopic({ ...topic, title: e.target.value })} /></label>
          <label>别名<input value={topic.aliases} onChange={e => setTopic({ ...topic, aliases: e.target.value })} placeholder="用逗号分隔" /></label>
          <label>正文<textarea rows={8} value={topic.content} readOnly={expanded} onChange={e => setTopic({ ...topic, content: e.target.value })} /></label>
          <div className="fox-contact-actions"><Button size="sm" disabled={busy}>保存专题</Button><Button type="button" variant="ghost" size="sm" onClick={() => setTopic(null)}>关闭</Button>
            {topic.id && <button type="button" onClick={() => setConfirm({ label: '永久删除专题“' + topic.title + '”及其检索索引？', command: 'companion_topic_delete', args: { topicId: topic.id, expectedSha: topic.contentSha256 } })}>删除专题…</button>}</div>
        </form>}
        {data.moves.filter(m => m.state === 'applied').map(m => <p key={m.id}>已移入 {data.topics.find(t => t.id === m.topicId)?.title} <button type="button" disabled={busy} onClick={() => void act('companion_note_move_undo', { moveId: m.id })}>撤销整理</button></p>)}
      </section>
      {data.requests.map(q => <section key={q.id} className="fox-contact-section"><h3>{q.action === 'remove' ? '确认删除笔记' : q.action === 'move' ? '确认移入专题' : '确认改写笔记'}</h3><p>{q.content}</p>{q.action === 'patch' && <p>改为：{q.payload?.content}</p>}{q.action === 'move' && <p>目标专题：{q.payload?.topic}</p>}<div className="fox-contact-actions">{[true, false].map(approve => <Button key={String(approve)} variant="outline" disabled={busy} onClick={() => void act('companion_note_request_resolve', { requestId: q.id, approve })}>{approve ? '同意' : '保留原文'}</Button>)}</div></section>)}
      {data.sharedRequests.map(q => <section key={q.id} className="fox-contact-section"><h3>确认共享记忆</h3>
        <SharedDecision request={q} busy={busy} onChoose={(decision, replaceMemoryId) => void act('companion_shared_request_resolve', { requestId: q.id, decision, replaceMemoryId })} /></section>)}
      {conflict && <section className="fox-contact-section"><h3>磁盘上的当前版本 · 第 {conflict.revision} 版</h3><p>{conflict.title}</p>
        <pre>{conflict.content}</pre><small>内容哈希 {conflict.contentSha256}</small>
        <Button size="sm" variant="outline" onClick={() => { setTopic({ ...conflict, aliases: conflict.aliases.join('，') }); setConflict(null); setError('') }}>载入当前版本</Button>
        {topic?.id === conflict.id && <Button size="sm" variant="outline" onClick={() => { setTopic({ ...topic, contentSha256: conflict.contentSha256 }); setConflict(null); setError('已保留编辑内容。请复核后保存，或重新确认删除。') }}>保留我的编辑并继续比较</Button>}</section>}
      {confirm && <div role="alert" className="fox-contact-section"><p>{confirm.label}</p><div className="fox-contact-actions"><Button variant="destructive" size="sm" disabled={busy} onClick={() => void act(confirm.command, confirm.args, () => { if (confirm.command === 'companion_topic_delete') setTopic(null); setConfirm(null) })}>确认删除</Button><Button variant="ghost" size="sm" onClick={() => setConfirm(null)}>取消</Button></div></div>}
    </>}
  </div>
}
