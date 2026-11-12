import { useState } from 'react'
import { ChevronRight, FolderOpen, MoreHorizontal, Pin, Plus, Search, UserRound } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { notify as toast } from '@/features/notifications'
import type { AgentRecord, ConversationDetail } from '@/features/conversations/model/types'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { contacts, type Companion } from './api'
import { useContacts, showContactError } from './context'
import { ContactDelete } from './delete'
import { ContactSearchBar, ContactSearchChip, ContactSearchPanel, type ContactChatSearch, useContactChatSearch } from './chat-search'
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
  const [creating, setCreating] = useState(false)
  const [expertOpen, setExpertOpen] = useState(false)
  const [experts, setExperts] = useState<AgentRecord[]>([])
  const [selectedExpertId, setSelectedExpertId] = useState('')
  const [expertsLoading, setExpertsLoading] = useState(false)
  if (!ctx) return null
  const { list, refresh } = ctx
  const open = async (contact: Companion, profile = false) => {
    try { ctx.open(await contacts.open(contact.id), profile) } catch (error) { showContactError(error) }
  }
  const create = async (expertId?: string) => {
    if (creating) return
    setCreating(true)
    try {
      const created = expertId ? await contacts.add(expertId) : await contacts.createBlank()
      const opened = await contacts.open(created.id)
      await refresh()
      ctx.open(opened)
      setExpertOpen(false)
      setSelectedExpertId('')
    } catch (error) { showContactError(error) } finally { setCreating(false) }
  }
  const chooseExpert = async () => {
    setExpertOpen(true)
    setExpertsLoading(true)
    try {
      const available = await desktopClient.listAgents()
      setExperts(available.filter(agent => agent.available && agent.runtimeType === 'pi' && agent.agentKind === 'expert'))
    } catch (error) { showContactError(error) } finally { setExpertsLoading(false) }
  }
  const archive = async (contact: Companion) => {
    try { await contacts.archive(contact.id); ctx.leave(contact); await refresh() } catch (error) { showContactError(error) }
  }
  if (collapsed) return <DropdownMenu><DropdownMenuTrigger asChild><button type="button" className="fox-sidebar-command" aria-label="Fox"><UserRound size={18} /></button></DropdownMenuTrigger>
    <DropdownMenuContent side="right">{list.companions.filter(item => !item.archivedAt && !item.deletingAt).map(item => <DropdownMenuItem key={item.id} onSelect={() => void open(item)}><ContactAvatar contact={item} />{item.displayName}</DropdownMenuItem>)}</DropdownMenuContent></DropdownMenu>
  return <section className="fox-contact-sidebar" aria-label="Fox">
    <div className="fox-section-head fox-sidebar-group-head"><span>小 Fox</span><DropdownMenu><DropdownMenuTrigger asChild><button type="button" className="fox-icon-button" aria-label="新建小 Fox" disabled={creating}><Plus size={14} /></button></DropdownMenuTrigger>
      <DropdownMenuContent align="end"><DropdownMenuItem disabled={creating} onSelect={() => void create()}>新建小 Fox</DropdownMenuItem><DropdownMenuItem disabled={creating} onSelect={() => void chooseExpert()}>从专家创建</DropdownMenuItem></DropdownMenuContent></DropdownMenu></div>
    {ctx.error && <button type="button" title={ctx.error} onClick={() => void refresh()}>重新加载联系人</button>}
    {list.companions.filter(item => !item.archivedAt && !item.deletingAt).map(item => <div className={'fox-contact-row' + (activeId === item.conversationId ? ' is-active' : '')} key={item.id}>
      <button type="button" className="fox-contact-link" onClick={() => void open(item)} aria-current={activeId === item.conversationId ? 'page' : undefined}><ContactAvatar contact={item} /><span>{item.displayName}</span>{item.pinnedAt && <Pin size={11} aria-label="已置顶" />}{item.pendingActionCount > 0 && <small className="fox-contact-flag is-action" aria-label="需要你确认">需要你确认</small>}{item.activeRunStatus && <small className="fox-contact-flag is-working">正在工作</small>}{item.unreadCount > 0 && <small className="fox-contact-flag is-unread" aria-label={item.unreadCount + ' 条新消息'}>{item.unreadCount}</small>}</button>
      <DropdownMenu><DropdownMenuTrigger asChild><button type="button" className="fox-icon-button" aria-label={'更多 ' + item.displayName + ' 操作'}><MoreHorizontal size={13} /></button></DropdownMenuTrigger>
        <DropdownMenuContent><DropdownMenuItem onSelect={() => void open(item, true)}>联系人资料</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void contacts.pin(item.id, !item.pinnedAt).then(refresh).catch(showContactError)}>{item.pinnedAt ? '取消置顶' : '置顶'}</DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void open(item, true)}>重命名</DropdownMenuItem>
          <DropdownMenuItem disabled={Boolean(item.activeRunStatus)} onSelect={() => void archive(item)}>归档</DropdownMenuItem>
          {!item.isDefault && <DropdownMenuItem variant="destructive" disabled={Boolean(item.activeRunStatus)} onSelect={() => setDeleting(item)}>删除…</DropdownMenuItem>}
        </DropdownMenuContent></DropdownMenu></div>)}
    {deleting && <ContactDelete contact={deleting} close={() => setDeleting(null)} />}
    <Dialog open={expertOpen} onOpenChange={open => { if (!creating) setExpertOpen(open) }}><DialogContent><DialogHeader><DialogTitle>从专家创建</DialogTitle><DialogDescription>选择一个可用的本机专家。每次确认都会创建独立的小 Fox。</DialogDescription></DialogHeader>
      <div role="radiogroup" aria-label="可用专家">{expertsLoading ? <p>正在加载专家…</p> : experts.length ? experts.map(agent => <label key={agent.id}><input type="radio" name="companion-expert" value={agent.id} checked={selectedExpertId === agent.id} disabled={creating} onChange={() => setSelectedExpertId(agent.id)} />{agent.name}</label>) : <p>暂无可用专家</p>}</div>
      <DialogFooter><Button variant="outline" disabled={creating} onClick={() => setExpertOpen(false)}>取消</Button><Button disabled={creating || !selectedExpertId} onClick={() => void create(selectedExpertId)}>{creating ? '正在创建…' : '确认创建'}</Button></DialogFooter>
    </DialogContent></Dialog>
  </section>
}

/**
 * The private-chat topbar. `search` lets the caller share ONE search instance with
 * the contact side card (SPEC item 3): when it is provided this component renders
 * the search UI but owns no search state, so the conversation can never end up with
 * two effects reacting to the same keyword or locate request. Without it the topbar
 * creates and owns its own instance.
 */
export function ContactTopbar(props: {
  detail: ConversationDetail | null
  loadEarlierMessages?: () => Promise<void>
  search?: ContactChatSearch
}) {
  const ctx = useContacts()
  if (props.search) return <ContactTopbarView search={props.search} contactName={ctx?.current?.displayName ?? '联系人'} />
  return <ContactTopbarOwningSearch detail={props.detail} loadEarlierMessages={props.loadEarlierMessages} />
}

function ContactTopbarOwningSearch({ detail, loadEarlierMessages }: {
  detail: ConversationDetail | null
  loadEarlierMessages?: () => Promise<void>
}) {
  const ctx = useContacts()
  const search = useContactChatSearch({
    conversationId: detail?.conversation?.id,
    detail,
    loadEarlierMessages: loadEarlierMessages ?? (async () => {}),
    locateRequest: ctx?.locateRequest,
  })
  return <ContactTopbarView search={search} contactName={ctx?.current?.displayName ?? '联系人'} />
}

function ContactTopbarView({ search, contactName }: { search: ContactChatSearch; contactName: string }) {
  const ctx = useContacts()
  const searching = search.mode !== 'closed'
  return <header className="fox-chat-topbar fox-contact-topbar"><span className="fox-contact-topbar-side" />
    {search.mode === 'panel'
      ? <ContactSearchBar search={search} />
      : search.mode === 'locating' || search.mode === 'chip'
        ? <ContactSearchChip search={search} />
        : <button type="button" className="fox-contact-heading" onClick={ctx?.showProfile}>
            {ctx?.current ? <ContactAvatar contact={ctx.current} /> : <UserRound size={17} />}<strong>{contactName}</strong><ChevronRight size={14} className="fox-contact-heading-chevron" aria-hidden="true" />
          </button>}
    <div className="fox-contact-topbar-side is-end">
      {!searching && <button type="button" className="fox-icon-button" title="搜索私聊" aria-label="搜索私聊" onClick={search.open}><Search size={16} /></button>}
    </div>
    {search.mode === 'panel' && <ContactSearchPanel search={search} contact={ctx?.current} contactName={contactName} />}
  </header>
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
