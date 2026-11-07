import { useState } from 'react'
import { FolderLock, FolderOpen, MoreHorizontal, PanelRight, Pin, Plus, UserRound } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { notify as toast } from '@/features/notifications'
import type { AgentRecord, ConversationDetail } from '@/features/conversations/model/types'
import { contacts, type Companion } from './api'
import { useContacts, showContactError } from './context'
import { ContactDelete } from './delete'
import './contacts.css'

export function ContactAvatar({ contact }: { contact: Companion }) {
  const avatar = contact.avatarRef
  return avatar && /^(\/|data:image\/)/.test(avatar)
    ? <img className="fox-contact-avatar" src={avatar} alt="" />
    : <span className="fox-contact-avatar is-letter" aria-hidden="true">{avatar && [...avatar].length <= 2 ? avatar : [...contact.displayName][0]}</span>
}

export function ContactSidebar({ activeId, collapsed = false }: { activeId?: string; collapsed?: boolean }) {
  const ctx = useContacts()
  const [deleting, setDeleting] = useState<Companion | null>(null)
  if (!ctx) return null
  const { list, refresh } = ctx
  const open = async (contact: Companion, profile = false) => {
    try { ctx.open(await contacts.open(contact.id), profile) } catch (error) { showContactError(error) }
  }
  const archive = async (contact: Companion) => {
    try { await contacts.archive(contact.id); ctx.leave(contact); await refresh() } catch (error) { showContactError(error) }
  }
  if (collapsed) return <DropdownMenu><DropdownMenuTrigger asChild><button type="button" className="fox-sidebar-command" aria-label="联系人"><UserRound size={18} /></button></DropdownMenuTrigger>
    <DropdownMenuContent side="right">{list.companions.filter(item => !item.archivedAt && !item.deletingAt).map(item => <DropdownMenuItem key={item.id} onSelect={() => void open(item)}><ContactAvatar contact={item} />{item.displayName}</DropdownMenuItem>)}<DropdownMenuItem onSelect={ctx.add}>添加联系人</DropdownMenuItem></DropdownMenuContent></DropdownMenu>
  return <section className="fox-contact-sidebar" aria-label="联系人">
    <div className="fox-section-head fox-sidebar-group-head"><span>联系人</span><button type="button" className="fox-icon-button" aria-label="添加联系人" onClick={ctx.add}><Plus size={14} /></button></div>
    {ctx.error && <button type="button" title={ctx.error} onClick={() => void refresh()}>重新加载联系人</button>}
    {list.companions.filter(item => !item.archivedAt && !item.deletingAt).map(item => <div className={'fox-contact-row' + (activeId === item.conversationId ? ' is-active' : '')} key={item.id}>
      <button type="button" className="fox-contact-link" onClick={() => void open(item)} aria-current={activeId === item.conversationId ? 'page' : undefined}><ContactAvatar contact={item} /><span>{item.displayName}</span>{item.pinnedAt && <Pin size={11} aria-label="已置顶" />}{item.activeRunStatus && <small>运行中</small>}{item.unreadCount > 0 && <small aria-label={item.unreadCount + ' 条未读'}>{item.unreadCount}</small>}</button>
      <DropdownMenu><DropdownMenuTrigger asChild><button type="button" className="fox-icon-button" aria-label={'更多 ' + item.displayName + ' 操作'}><MoreHorizontal size={13} /></button></DropdownMenuTrigger>
        <DropdownMenuContent><DropdownMenuItem onSelect={() => void open(item, true)}>联系人资料</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void contacts.pin(item.id, !item.pinnedAt).then(refresh).catch(showContactError)}>{item.pinnedAt ? '取消置顶' : '置顶'}</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void open(item, true)}>重命名</DropdownMenuItem>
          <DropdownMenuItem disabled={Boolean(item.activeRunStatus)} onSelect={() => void archive(item)}>归档</DropdownMenuItem>
          {!item.isDefault && <DropdownMenuItem variant="destructive" disabled={Boolean(item.activeRunStatus)} onSelect={() => setDeleting(item)}>删除…</DropdownMenuItem>}
        </DropdownMenuContent></DropdownMenu></div>)}
    {deleting && <ContactDelete contact={deleting} close={() => setDeleting(null)} />}
  </section>
}

export function AddContactAction({ agent, onOpened }: { agent: AgentRecord; onOpened?: () => void }) {
  const ctx = useContacts()
  const [busy, setBusy] = useState(false)
  if (!ctx || agent.agentKind !== 'expert' || agent.runtimeType !== 'pi') return null
  const existing = ctx.list.companions.find(item => item.templateAgentId === agent.id)
  const add = async () => {
    setBusy(true)
    try {
      const result = await contacts.add(agent.id)
      await ctx.refresh(); onOpened?.(); ctx.open(result)
    } catch (e) { showContactError(e) } finally { setBusy(false) }
  }
  return <Button variant="outline" disabled={busy || Boolean(existing?.deletingAt)} onClick={() => void add()}>{existing?.deletingAt ? '正在删除' : existing?.archivedAt ? '恢复并打开' : existing ? '打开私聊' : '加为联系人'}</Button>
}

export function ContactComposerFooter() {
  const ctx = useContacts()
  if (!ctx?.privateChat) return null
  return <div className="fox-composer-draft-footer fox-contact-footer">
    <button type="button" className="fox-composer-project-trigger" title={ctx.current?.workspaceDir} onClick={ctx.showProfile}><FolderLock size={13}/><span>工作目录</span></button>
    <button type="button" className="fox-composer-project-trigger" onClick={ctx.showProfile}>{ctx.current && <ContactAvatar contact={ctx.current}/>}<span>{ctx.current?.displayName ?? '联系人'}</span></button>
  </div>
}

export function ContactTopbar({ detail, onReset }: {
  detail: ConversationDetail | null
  onReset: () => void
}) {
  const ctx = useContacts()
  return <header className="fox-chat-topbar fox-contact-topbar"><button type="button" className="fox-contact-heading" onClick={ctx?.showProfile}>
    {ctx?.current ? <ContactAvatar contact={ctx.current} /> : <UserRound size={17} />}<strong>{ctx?.current?.displayName ?? '联系人'}</strong>
    </button>
    {detail?.contextReset?.supported && <Button
      variant="ghost" size="sm" disabled={Boolean(detail.contextResetBlockedReason || ctx?.current?.activeRunStatus)}
      title={detail.contextResetBlockedReason ?? undefined} onClick={onReset}
    >重置上下文</Button>}
    <button type="button" className="fox-icon-button" title="联系人资料" aria-label="联系人资料" onClick={ctx?.showProfile}><PanelRight size={16} /></button></header>
}

export function ContactRepairBanner({ detail }: { detail: ConversationDetail | null }) {
  const ctx = useContacts()
  const [dismissed, setDismissed] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const contact = ctx?.current
  if (!ctx || !contact || !detail) return null
  const workspace = contact.workspaceState !== 'ready'
  const damaged = detail.contextRepair?.available && dismissed !== detail.conversation.id
  if (!workspace && !damaged) return null
  const repair = async () => {
    setBusy(true)
    try {
      if (workspace) await contacts.repairWorkspace(contact.id)
      else await contacts.repairContext(contact.conversationId)
      await ctx.refresh(); await ctx.refreshConversation(); toast.success(workspace ? '工作目录已准备好' : '私聊已修复，可以继续发送消息')
    } catch (error) { showContactError(error) } finally { setBusy(false) }
  }
  return <aside className="fox-contact-banner" role="status"><FolderOpen size={16} />
    <div>{workspace ? contact.workspaceError || '工作目录不可用' : '这段对话的模型上下文已损坏。修复后，小 Fox 将忘记此前对话的细节。聊天记录、笔记、项目授权和工作文件均保留。'}
      <div><Button size="sm" disabled={busy} onClick={() => void repair()}>{workspace ? '重新创建工作目录' : '修复后继续'}</Button>
        {!workspace && <Button size="sm" variant="ghost" onClick={() => setDismissed(detail.conversation.id)}>暂不修复</Button>}</div></div></aside>
}
