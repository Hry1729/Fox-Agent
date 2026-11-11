import { useCallback, useEffect, useState } from 'react'
import { FolderOpen, FolderPlus, FolderSearch, RotateCcw, Search, Settings2, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { notify as toast } from '@/features/notifications'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ConversationDetail, ProjectFileEntry, ProjectRecord } from '@/features/conversations/model/types'
import { contacts, type Companion, type ReminderMode, type TemplateStatus } from './api'
import { ContactAvatar } from './chat-controls'
import { ContactModelSelect } from './model-select'
import { ContactFiles } from './files'
import { ContactCapabilities } from './capabilities'
import { ContactFavorites } from './marks'
import { ContactNotes } from './notes'
import { ContactDelete } from './delete'
import { ContactGrants, TemplateChanges } from './profile'
import type { ContactChatSearch } from './chat-search'
import { useContacts, showContactError } from './context'
import './contact-side-card.css'

/** Fixed card width; the shared window-growth controller uses it as the delta. */
export const CONTACT_SIDE_CARD_WIDTH = 320

export type ContactSideCardTab = 'details' | 'files' | 'workspace'
const TAB_LABELS: Array<[ContactSideCardTab, string]> = [['details', '详情'], ['files', '文件与成果'], ['workspace', '工作目录']]
const REMINDER_OPTIONS: Array<[ReminderMode, string]> = [
  ['follow_global', '跟随全局设置'],
  ['all', '所有消息'],
  ['action_only', '只有需要我确认的消息'],
  ['muted', '暂时不提醒'],
]
/** The real contract: `permissionMode` is read_only / ask / allow (see profile.tsx
 *  and the composer), and it is applied through `ctx.setPermission`. */
const PERMISSION_OPTIONS: Array<[ProjectRecord['permissionMode'], string]> = [
  ['read_only', '只读'],
  ['ask', '每轮询问'],
  ['allow', '自动接受'],
]

/** The private-chat contact card (SPEC item 3): an independent 320px sibling card
 * that replaces the old RightPanel「联系人资料」route. It reuses Fox's own existing
 * blocks — model, reset context, search-in-chat, reminders, permission, shared
 * memory, capabilities, favorites, notes, files, workspace, and a settings subview
 * for the profile editing / template / grants / pin-archive-delete the old profile
 * page owned — so redesigning the page never removed a feature. */
export function ContactSideCard({ contact, detail, onClose, onResetContext, search }: {
  contact: Companion
  detail: ConversationDetail | null
  onClose: () => void
  onResetContext: () => void
  /** Shared search instance: 「在对话中查找」 opens the topbar search panel. */
  search?: ContactChatSearch
}) {
  const ctx = useContacts()
  const [tab, setTab] = useState<ContactSideCardTab>('details')
  const [view, setView] = useState<'main' | 'capabilities' | 'favorites' | 'notes' | 'settings'>('main')
  const [renaming, setRenaming] = useState(false)
  const [nameDraft, setNameDraft] = useState('')
  const [busy, setBusy] = useState(false)
  // Workspace file list is read from the CONVERSATION (the project files API takes
  // a conversation id), and 「导出文件」 picks one FILE's relative path.
  const [files, setFiles] = useState<ProjectFileEntry[]>([])
  const [filesOpen, setFilesOpen] = useState(false)
  const [profileDraft, setProfileDraft] = useState({ avatar: contact.avatarRef, duty: contact.duty, persona: contact.personaOverride ?? '' })
  const [template, setTemplate] = useState<TemplateStatus | null>(null)
  const [deleting, setDeleting] = useState(false)
  const run = async (action: () => Promise<unknown>, ok?: string) => {
    setBusy(true)
    try {
      await action()
      await ctx?.refresh()
      if (ok) toast.success(ok)
    } catch (error) { showContactError(error) } finally { setBusy(false) }
  }
  const commitRename = async () => {
    const next = nameDraft.trim()
    setRenaming(false)
    if (!next || next === contact.displayName) return
    await run(
      () => contacts.profile(contact.id, { displayName: next, avatarRef: contact.avatarRef, duty: contact.duty, personaOverride: contact.personaOverride }),
      '已保存联系人资料',
    )
  }
  const loadFiles = useCallback(async () => {
    setBusy(true)
    try {
      setFiles((await desktopClient.listProjectFiles(contact.conversationId)).filter(file => !file.isDirectory))
      setFilesOpen(true)
    } catch (error) { showContactError(error) } finally { setBusy(false) }
  }, [contact.conversationId])
  const loadTemplate = useCallback(async () => {
    const next = await contacts.template(contact.id)
    setTemplate(next)
    return next
  }, [contact.id])
  // The template diff is only needed in the settings subview, so the card does not
  // spend an IPC round trip on every open.
  useEffect(() => { if (view === 'settings' && template === null) void loadTemplate().catch(showContactError) }, [view, template, loadTemplate])
  const decideTemplate = async (adopt: boolean) => {
    if (!template?.latestHash) return
    setBusy(true)
    try {
      await contacts.decideTemplate(contact.id, template.latestHash, adopt)
      // Re-read the status so the card shows the Host's post-decision state instead
      // of the stale diff it acted on.
      await loadTemplate()
      if (adopt) await ctx?.refreshConversation()
    } catch (error) { showContactError(error) } finally { setBusy(false) }
  }
  const heading = view === 'capabilities' ? '可用能力' : view === 'favorites' ? '收藏' : view === 'notes' ? '笔记' : view === 'settings' ? '联系人设置' : '联系人资料'
  const resetBlocked = Boolean(detail?.contextResetBlockedReason || contact.activeRunStatus)
  return <aside className="fox-contact-side-card" role="complementary" aria-label={`${contact.displayName} 资料卡`}>
    <header className="fox-contact-side-card-head">
      <h2>{heading}</h2>
      <button type="button" className="fox-contact-side-card-close" aria-label="关闭联系人资料卡" title="关闭" onClick={onClose}><X size={14} /></button>
    </header>
    <div className="fox-contact-side-card-body">
      {view !== 'main'
        ? <div className="fox-contact-side-view">
            <div className="fox-contact-side-back"><Button size="sm" variant="ghost" onClick={() => setView('main')}>返回详情</Button></div>
            {view === 'capabilities' && <ContactCapabilities key={contact.id} contact={contact} />}
            {view === 'favorites' && <ContactFavorites key={contact.id} contact={contact} />}
            {view === 'notes' && <ContactNotes key={contact.id} contact={contact} />}
            {view === 'settings' && <div className="fox-contact-panel fox-contact-side-settings"><ContactSideSettings
              contact={contact}
              busy={busy}
              draft={profileDraft}
              onDraft={setProfileDraft}
              onSave={fields => void run(() => contacts.profile(contact.id, { displayName: contact.displayName, ...fields }), '已保存联系人资料')}
              template={template}
              onTemplate={decideTemplate}
              onPin={() => void run(() => contacts.pin(contact.id, !contact.pinnedAt))}
              onArchive={() => void run(async () => { await contacts.archive(contact.id); ctx?.leave(contact) })}
              onDelete={() => setDeleting(true)}
            /></div>}
            {deleting && <ContactDelete contact={contact} close={() => setDeleting(false)} />}
          </div>
        : <>
            <div className="fox-contact-side-hero">
              <ContactAvatar contact={contact} />
              {renaming
                ? <input
                    aria-label="重命名联系人"
                    value={nameDraft}
                    maxLength={40}
                    autoFocus
                    onChange={event => setNameDraft(event.target.value)}
                    onBlur={() => void commitRename()}
                    onKeyDown={event => {
                      if (event.key === 'Enter') { event.preventDefault(); void commitRename() }
                      else if (event.key === 'Escape') { event.preventDefault(); setRenaming(false) }
                    }}
                  />
                : <button type="button" className="fox-contact-side-hero-name" title="重命名"
                    onClick={() => { setNameDraft(contact.displayName); setRenaming(true) }}>{contact.displayName}</button>}
            </div>
            <div className="fox-contact-side-tabs" role="tablist" aria-label="资料卡分页">
              {TAB_LABELS.map(([value, label]) => <button key={value} type="button" role="tab" aria-selected={tab === value}
                className={tab === value ? 'is-selected' : undefined} onClick={() => setTab(value)}>{label}</button>)}
            </div>
            {tab === 'details' && <div className="fox-contact-panel fox-contact-side-panel">
              <div className="fox-contact-section">
                <label><span>模型</span><ContactModelSelect key={contact.id} contact={contact} /></label>
                {detail?.contextReset?.supported && <div className="fox-contact-side-row">
                  <Button size="sm" variant="outline" disabled={resetBlocked} title={detail.contextResetBlockedReason ?? undefined} onClick={onResetContext}>
                    <RotateCcw size={13} />重置上下文
                  </Button>
                  <small>{detail.contextResetBlockedReason ?? '清除模型记忆，但保留聊天记录、工作文件和项目授权'}</small>
                </div>}
                {search && <Button size="sm" variant="outline" onClick={search.open}><Search size={13} />在对话中查找</Button>}
              </div>
              <div className="fox-contact-section">
                <label><span>提醒</span>
                  <select value={contact.reminderMode} aria-label="提醒" disabled={busy}
                    onChange={event => void run(async () => { await contacts.setReminderMode(contact.id, event.target.value as ReminderMode) }, '提醒设置已保存')}>
                    {REMINDER_OPTIONS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
                  </select></label>
                <label><span>权限</span>
                  <select value={ctx?.permission ?? 'ask'} aria-label="权限" disabled={busy}
                    onChange={event => ctx?.setPermission(event.target.value as ProjectRecord['permissionMode'])}>
                    {PERMISSION_OPTIONS.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
                  </select></label>
                <Button size="sm" variant="outline" onClick={ctx?.openSharedMemory}>共享记忆</Button>
              </div>
              <div className="fox-contact-section">
                <Button size="sm" variant="outline" onClick={() => setView('capabilities')}>可用能力</Button>
                <Button size="sm" variant="outline" onClick={() => setView('favorites')}>收藏</Button>
                <Button size="sm" variant="outline" onClick={() => setView('notes')}>笔记</Button>
                <Button size="sm" variant="outline" onClick={() => setView('settings')}><Settings2 size={13} />设置</Button>
              </div>
            </div>}
            {tab === 'files' && <div className="fox-contact-side-view"><ContactFiles key={contact.id} contact={contact} /></div>}
            {tab === 'workspace' && <div className="fox-contact-panel fox-contact-side-panel">
              <div className="fox-contact-section">
                <code>{contact.workspaceDir}</code>
                {contact.workspaceError && <p role="alert">{contact.workspaceError}</p>}
                <div className="fox-contact-actions">
                  <Button size="sm" variant="outline" disabled={busy}
                    onClick={() => void run(() => desktopClient.openProjectFolder(contact.conversationId))}><FolderOpen size={13} />打开文件夹</Button>
                  {contact.workspaceState !== 'ready' && <Button size="sm" variant="outline" disabled={busy}
                    onClick={() => void run(async () => { await contacts.repairWorkspace(contact.id) }, '工作目录已准备好')}><FolderSearch size={13} />重新创建工作目录</Button>}
                  <Button size="sm" variant="outline" disabled={busy}
                    onClick={() => void run(async () => {
                      const paths = await contacts.importFiles(contact.id)
                      if (!paths.length) return
                      toast.success('已复制 ' + paths.length + ' 个文件')
                      if (filesOpen) await loadFiles()
                    })}><FolderPlus size={13} />添加文件…</Button>
                  <Button size="sm" variant="outline" disabled={busy} onClick={() => void loadFiles()}><FolderOpen size={13} />导出文件…</Button>
                </div>
                {filesOpen && (files.length
                  ? <div className="fox-contact-side-export-list">{files.map(file => <button key={file.path} type="button"
                      disabled={busy} onClick={() => void run(() => contacts.exportFile(contact.id, file.path))}>{file.path} · 另存为</button>)}</div>
                  : <p>工作目录里还没有文件。</p>)}
              </div>
            </div>}
          </>}
    </div>
  </aside>
}

/** The settings subview: the fields and actions the old profile page owned, in
 *  smaller reused blocks. `ContactProfile` itself is the whole old page layout and
 *  cannot be embedded here without re-introducing the page the SPEC replaces. */
function ContactSideSettings({ contact, busy, draft, onDraft, onSave, template, onTemplate, onPin, onArchive, onDelete }: {
  contact: Companion
  busy: boolean
  draft: { avatar: string; duty: string; persona: string }
  onDraft: (next: { avatar: string; duty: string; persona: string }) => void
  onSave: (fields: { avatarRef: string; duty: string; personaOverride: string }) => void
  template: TemplateStatus | null
  onTemplate: (adopt: boolean) => void
  onPin: () => void
  onArchive: () => void
  onDelete: () => void
}) {
  return <>
    <form className="fox-contact-section" onSubmit={event => { event.preventDefault(); onSave({ avatarRef: draft.avatar, duty: draft.duty, personaOverride: draft.persona }) }}>
      <label><span>头像</span><input aria-label="头像" value={draft.avatar} placeholder="输入表情或头像资源路径" onChange={event => onDraft({ ...draft, avatar: event.target.value })} /></label>
      <label><span>职责</span><textarea aria-label="职责" value={draft.duty} onChange={event => onDraft({ ...draft, duty: event.target.value })} /></label>
      <label><span>自定义人设</span><textarea aria-label="自定义人设" value={draft.persona} placeholder="留空时使用模板人设" onChange={event => onDraft({ ...draft, persona: event.target.value })} /></label>
      <Button type="submit" size="sm" disabled={busy}>保存资料</Button>
    </form>
    <section className="fox-contact-section">
      <h3>专家模板</h3>
      {template?.boundSnapshot?.packageVersion != null && <small>当前版本 {String(template.boundSnapshot.packageVersion)}</small>}
      <p>{contact.isDefault ? '小狐的模板随 Fox 更新' : template?.updateAvailable ? '专家模板有新版' : '正在使用当前模板'}</p>
      {template?.updateAvailable && <details open={!template.dismissed}><summary>查看变化</summary>
        <TemplateChanges template={template} />
        <div className="fox-contact-actions">
          <Button size="sm" disabled={busy || Boolean(contact.activeRunStatus)} onClick={() => onTemplate(true)}>采用新版</Button>
          <Button size="sm" variant="ghost" disabled={busy || template.dismissed} onClick={() => onTemplate(false)}>暂不更新</Button>
        </div></details>}
    </section>
    {!contact.isDefault && <ContactGrants key={contact.id} contact={contact} />}
    <section className="fox-contact-section">
      <div className="fox-contact-actions">
        <Button size="sm" variant="outline" disabled={busy} onClick={onPin}>{contact.pinnedAt ? '取消置顶' : '置顶'}</Button>
        <Button size="sm" variant="outline" disabled={busy || Boolean(contact.activeRunStatus)} onClick={onArchive}>归档</Button>
        {!contact.isDefault && <Button size="sm" variant="ghost" disabled={busy || Boolean(contact.activeRunStatus)} onClick={onDelete}>删除…</Button>}
      </div>
    </section>
  </>
}
