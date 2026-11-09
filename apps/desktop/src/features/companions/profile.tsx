import { useCallback, useEffect, useState } from 'react'
import { SettingsGroup, SettingsRow } from '@/features/settings/settings-layout'
import { Button } from '@/components/ui/button'
import { notify as toast } from '@/features/notifications'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { ProjectFileEntry } from '@/features/conversations/model/types'
import { contacts, direct, type Companion, type CompanionGrant, type ReminderMode, type RetainedWorkspace, type TemplateStatus } from './api'
import { ContactCapabilities } from './capabilities'
import { ContactFiles } from './files'
import { ContactFavorites } from './marks'
import { showContactError, useContacts } from './context'
import { ContactDelete } from './delete'
import { ContactModelSelect } from './model-select'
import { ContactNotes } from './notes'

function TemplateChanges({ template }: { template: TemplateStatus }) {
  const before = template.boundSnapshot ?? {}
  const after = template.latestSnapshot ?? {}
  const rows = [
    ['名称', before.name, after.name],
    ['简介', before.description, after.description],
    ['工作要求', before.systemPrompt, after.systemPrompt],
    ['开场建议', before.openingSuggestions?.join('、'), after.openingSuggestions?.join('、')],
    ['技能', before.packageManifest?.skills?.join('、'), after.packageManifest?.skills?.join('、')],
    ['可用工具', before.packageManifest?.allowedTools?.join('、'), after.packageManifest?.allowedTools?.join('、')],
    ['连接器', before.packageManifest?.mcpServers?.join('、'), after.packageManifest?.mcpServers?.join('、')],
    ['版本', before.packageVersion, after.packageVersion],
  ].filter(([, oldValue, newValue]) => oldValue !== newValue)
  return rows.length ? <>{rows.map(([label, oldValue, newValue]) => <div key={label}>
    <strong>{label}</strong><p>当前：{oldValue || '未设置'}</p><p>新版：{newValue || '未设置'}</p>
  </div>)}</> : <p>专家模板的配置已更新。</p>
}

export function ContactProfile({ contact }: { contact: Companion }) {
  const ctx = useContacts()
  const [name, setName] = useState(contact.displayName)
  const [avatar, setAvatar] = useState(contact.avatarRef)
  const [duty, setDuty] = useState(contact.duty)
  const [persona, setPersona] = useState(contact.personaOverride ?? '')
  const [deleting, setDeleting] = useState(false)
  const [tab, setTab] = useState<'profile' | 'notes'>('profile')
  const [capabilitiesOpen, setCapabilitiesOpen] = useState(false)
  const [libraryOpen, setLibraryOpen] = useState(false)
  const [favoritesOpen, setFavoritesOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [files, setFiles] = useState<ProjectFileEntry[]>([])
  const [filesOpen, setFilesOpen] = useState(false)
  const [template, setTemplate] = useState<TemplateStatus | null>(null)
  const loadTemplate = useCallback(() => contacts.template(contact.id).then(setTemplate), [contact.id])
  useEffect(() => { void loadTemplate().catch(showContactError) }, [loadTemplate])
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true)
    try { await action(); await ctx?.refresh() } catch (cause) { showContactError(cause) } finally { setBusy(false) }
  }
  const loadFiles = async () => setFiles((await desktopClient.listProjectFiles(contact.conversationId)).filter(f => !f.isDirectory))
  const decide = async (adopt: boolean) => {
    if (!template?.latestHash) return
    await contacts.decideTemplate(contact.id, template.latestHash, adopt)
    await loadTemplate()
    if (adopt) await ctx?.refreshConversation()
  }
  return <div className="fox-contact-panel"><div className="fox-contact-actions" role="group" aria-label="资料分类">
    <Button size="sm" variant={tab === 'profile' ? 'secondary' : 'ghost'} onClick={() => { setTab('profile'); setCapabilitiesOpen(false); setLibraryOpen(false); setFavoritesOpen(false) }}>资料</Button>
    <Button size="sm" variant={tab === 'notes' ? 'secondary' : 'ghost'} onClick={() => setTab('notes')}>笔记</Button></div>
    {tab === 'notes' ? <ContactNotes contact={contact} /> : capabilitiesOpen ? <ContactCapabilities contact={contact} onBack={() => setCapabilitiesOpen(false)} /> : libraryOpen ? <ContactFiles contact={contact} onBack={() => setLibraryOpen(false)} /> : favoritesOpen ? <ContactFavorites contact={contact} onBack={() => setFavoritesOpen(false)} /> : <>
      <form className="fox-contact-section" onSubmit={e => { e.preventDefault(); void run(async () => { await contacts.profile(contact.id, { displayName: name, avatarRef: avatar, duty, personaOverride: persona }); toast.success('已保存联系人资料') }) }}>
        <label>名字<input value={name} required maxLength={40} onChange={e => setName(e.target.value)} /></label>
        <label>头像<input value={avatar} onChange={e => setAvatar(e.target.value)} placeholder="输入表情或头像资源路径" /></label>
        <label>职责<textarea value={duty} onChange={e => setDuty(e.target.value)} /></label>
        <label>自定义人设<textarea value={persona} onChange={e => setPersona(e.target.value)} placeholder="留空时使用模板人设" /></label>
        <Button type="submit" size="sm" disabled={busy}>保存资料</Button>
      </form>
      <section className="fox-contact-section"><h3>专家模板</h3>
        {template?.boundSnapshot?.packageVersion != null && <small>当前版本 {String(template.boundSnapshot.packageVersion)}</small>}
        <p>{contact.isDefault ? '小狐的模板随 Fox 更新' : template?.updateAvailable ? '专家模板有新版' : '正在使用当前模板'}</p>
        {template?.updateAvailable && <details open={!template.dismissed}><summary>查看变化</summary>
          <TemplateChanges template={template} />
          <div className="fox-contact-actions"><Button size="sm" disabled={busy || Boolean(contact.activeRunStatus)} onClick={() => void run(() => decide(true))}>采用新版</Button><Button size="sm" variant="ghost" disabled={busy || template.dismissed} onClick={() => void run(() => decide(false))}>暂不更新</Button></div></details>}
      </section>
      <SettingsGroup title="对话设置"><SettingsRow title="模型"><ContactModelSelect contact={contact} /></SettingsRow>
        <SettingsRow title="提醒" description="静音只关闭主动弹窗和声音，侧栏未读与待处理标记仍保留"><select aria-label="提醒方式" value={contact.reminderMode} disabled={busy} onChange={e => void run(() => contacts.setReminderMode(contact.id, e.target.value as ReminderMode))}><option value="follow_global">沿用全局</option><option value="all">全部提醒</option><option value="action_only">仅需要处理</option><option value="muted">静音</option></select></SettingsRow>
        <SettingsRow title="权限"><select aria-label="私聊权限" value={ctx?.permission ?? 'ask'} onChange={e => ctx?.setPermission(e.target.value as 'read_only' | 'ask' | 'allow')}><option value="read_only">只读</option><option value="ask">询问</option><option value="allow">自动</option></select></SettingsRow>
        <SettingsRow title="共享记忆"><Button size="sm" variant="ghost" onClick={ctx?.openSharedMemory}>查看共享记忆</Button></SettingsRow>
        <SettingsRow title="可用能力" description="网页搜索、读取网页和连接器工具"><Button size="sm" variant="ghost" onClick={() => { setCapabilitiesOpen(true); setLibraryOpen(false); setFavoritesOpen(false) }}>查看和设置</Button></SettingsRow>
        <SettingsRow title="文件与成果" description="我提供的文件和联系人生成的结果"><Button size="sm" variant="ghost" onClick={() => { setLibraryOpen(true); setCapabilitiesOpen(false); setFavoritesOpen(false) }}>查看</Button></SettingsRow>
        <SettingsRow title="收藏" description="只是你自己的标记，不会写进它的笔记"><Button size="sm" variant="ghost" onClick={() => { setFavoritesOpen(true); setCapabilitiesOpen(false); setLibraryOpen(false) }}>查看收藏</Button></SettingsRow></SettingsGroup>
      <section className="fox-contact-section"><h3>工作目录</h3><code>{contact.workspaceDir}</code>
        {contact.workspaceError && <p role="alert">{contact.workspaceError}</p>}
        <div className="fox-contact-actions"><Button size="sm" variant="outline" onClick={() => void run(() => desktopClient.openProjectFolder(contact.conversationId))}>打开文件夹</Button>
          {contact.workspaceState !== 'ready' && <Button size="sm" onClick={() => void run(() => contacts.repairWorkspace(contact.id))}>重新创建工作目录</Button>}
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void run(async () => { const paths = await contacts.importFiles(contact.id); if (paths.length) { toast.success('已复制 ' + paths.length + ' 个文件'); if (filesOpen) await loadFiles() } })}>加入文件</Button>
          <Button size="sm" variant="outline" onClick={() => void run(async () => { await loadFiles(); setFilesOpen(true) })}>导出文件</Button></div>
        {filesOpen && (files.length ? files.map(file => <button type="button" key={file.path} onClick={() => void run(() => contacts.exportFile(contact.id, file.path))}>{file.path} · 另存为</button>) : <p>工作目录里还没有文件。</p>)}
      </section>
      {!contact.isDefault && <ContactGrants contact={contact} />}
      <section className="fox-contact-section"><div className="fox-contact-actions"><Button variant="outline" size="sm" disabled={busy} onClick={() => void run(() => contacts.pin(contact.id, !contact.pinnedAt))}>{contact.pinnedAt ? '取消置顶' : '置顶'}</Button>
        <Button variant="outline" size="sm" disabled={busy || Boolean(contact.activeRunStatus)} onClick={() => void run(async () => { await contacts.archive(contact.id); ctx?.leave(contact) })}>归档</Button>
        {!contact.isDefault && <Button variant="ghost" size="sm" disabled={busy || Boolean(contact.activeRunStatus)} onClick={() => setDeleting(true)}>删除…</Button>}</div></section>
      {deleting && <ContactDelete contact={contact} close={() => setDeleting(false)} />}</>}
  </div>
}

export function ContactGrants({ contact, onlyProject }: { contact: Companion; onlyProject?: string }) {
  const ctx = useContacts()
  const [grants, setGrants] = useState<CompanionGrant[]>([])
  const [busy, setBusy] = useState(true)
  const [error, setError] = useState('')
  const reload = useCallback(async () => {
    setBusy(true)
    try { setGrants(await contacts.grants(contact.id)); setError('') } catch (e) { setError(desktopErrorDetails(e).message) } finally { setBusy(false) }
  }, [contact.id])
  useEffect(() => { void reload() }, [reload])
  const change = async (projectId: string, grant: boolean) => {
    setBusy(true)
    try { await contacts.setGrant(contact.id, projectId, grant); await reload() } catch (e) { showContactError(e); setBusy(false) }
  }
  const projects = (ctx?.projects ?? []).filter(p => p.status === 'active' && (!onlyProject || p.id === onlyProject))
  return <section className="fox-contact-section"><h3>{onlyProject ? contact.displayName : '项目笔记授权'}</h3>
    {!onlyProject && <p>只有获准的项目对话引入此专家时，才能读取主笔记和专题。</p>}
    {error ? <button type="button" onClick={() => void reload()}>{error} · 重试</button> : !projects.length ? <p>没有可授权的项目。</p> : projects.map(p => <label key={p.id} style={{ display: 'flex' }}><input type="checkbox" checked={grants.some(g => g.projectId === p.id)} disabled={busy} onChange={e => void change(p.id, e.target.checked)} />{onlyProject ? '允许读取联系人笔记' : p.name}</label>)}
  </section>
}

export function ProjectContactGrants({ projectId }: { projectId: string }) {
  const ctx = useContacts()
  const items = ctx?.list.companions.filter(c => !c.isDefault && !c.deletingAt)
  return items?.length ? <details><summary>联系人笔记授权</summary>{items.map(c => <ContactGrants key={c.id} contact={c} onlyProject={projectId} />)}</details> : null
}

export function ContactSettings() {
  const ctx = useContacts()
  const [retained, setRetained] = useState<RetainedWorkspace[]>([])
  const [error, setError] = useState('')
  const refresh = useCallback(async () => { try { setRetained(await contacts.retained()); setError('') } catch (e) { setError(desktopErrorDetails(e).message) } }, [])
  useEffect(() => { void refresh() }, [refresh])
  if (!ctx) return null
  const act = async (f: () => Promise<unknown>) => { try { await f(); await ctx.refresh(); await refresh() } catch (e) { showContactError(e) } }
  const archived = ctx.list.companions.filter(c => c.archivedAt && !c.deletingAt)
  return <section className="fox-contact-panel"><h3>已归档的联系人</h3>
    {!archived.length && <p>暂无归档联系人。</p>}{archived.map(c => <div className="fox-contact-entry" key={c.id}>{c.displayName} <Button variant="outline" size="sm" onClick={() => void act(async () => { const restored = await contacts.restore(c.id); if (restored.displayName !== c.displayName) toast.info('恢复为 ' + restored.displayName) })}>恢复</Button></div>)}
    {ctx.list.pendingDeletions.length > 0 && <section className="fox-contact-section"><h3>删除未完成</h3>{ctx.list.pendingDeletions.map(item => <p key={item.companionId}>{item.displayName} {item.error}<Button onClick={() => void act(() => contacts.resume(item.companionId))}>继续删除</Button></p>)}</section>}
    <section className="fox-contact-section"><h3>删除后保留的工作目录</h3>{error && <button type="button" onClick={() => void refresh()}>{error} · 重试</button>}
      {!error && !retained.length && <p>暂无保留目录。</p>}{retained.map(item => <div key={item.companionId}><p>{item.displayName} · {item.workspacePath}</p><div className="fox-contact-actions">
        <Button size="sm" variant="outline" onClick={() => void act(() => direct('companion_retained_open', { companionId: item.companionId }))}>打开文件夹</Button>
        <Button size="sm" variant="ghost" onClick={() => void act(() => direct('companion_retained_dismiss', { companionId: item.companionId }))}>从列表移除</Button></div></div>)}</section>
  </section>
}
