import { useEffect, useRef, useState } from 'react'
import { Archive, ChevronRight, FolderOpen, MoreHorizontal, Pencil, Pin, PinOff, Plus, Search, Trash2, UserRound } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { notify as toast } from '@/features/notifications'
import type { AgentRecord, ConversationDetail } from '@/features/conversations/model/types'
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
  if (!ctx) return null
  const { list, refresh } = ctx
  const open = async (contact: Companion, profile = false) => {
    try { ctx.open(await contacts.open(contact.id), profile) } catch (error) { showContactError(error) }
  }
  const archive = async (contact: Companion) => {
    try { await contacts.archive(contact.id); ctx.leave(contact); await refresh() } catch (error) { showContactError(error) }
  }
  if (collapsed) return <DropdownMenu><DropdownMenuTrigger asChild><Button type="button" variant="ghost" size="icon" className="fox-sidebar-command" aria-label="Fox"><UserRound size={18} /></Button></DropdownMenuTrigger>
    <DropdownMenuContent side="right" className="fox-conversation-menu">{list.companions.filter(item => !item.archivedAt && !item.deletingAt).map(item => <DropdownMenuItem key={item.id} onSelect={() => void open(item)}><ContactAvatar contact={item} />{item.displayName}</DropdownMenuItem>)}<DropdownMenuItem onSelect={() => runAfterMenuClose(ctx.add)}><Plus />添加联系人</DropdownMenuItem></DropdownMenuContent></DropdownMenu>
  return <section className="fox-contact-sidebar" aria-label="Fox">
    <div className="fox-section-head fox-sidebar-group-head"><span>Fox</span><Button type="button" variant="ghost" size="icon" className="fox-icon-button" aria-label="添加联系人" onClick={ctx.add}><Plus size={14} /></Button></div>
    {ctx.error && <button type="button" title={ctx.error} onClick={() => void refresh()}>重新加载联系人</button>}
    {list.companions.filter(item => !item.archivedAt && !item.deletingAt).map(item => {
      const preview = item.latestMessagePreview?.replace(/\s+/g, ' ').trim()
      const previewId = preview ? `fox-contact-preview-${item.id}` : undefined
      return <div className={'fox-contact-row' + (activeId === item.conversationId ? ' is-active' : '')} key={item.id}>
        <ContactAvatar contact={item} />
        <div className="fox-contact-row-copy">
          <div className="fox-contact-row-head">
            <button type="button" className="fox-contact-link" onClick={() => void open(item)}
              aria-current={activeId === item.conversationId ? 'page' : undefined} aria-describedby={previewId}>
              <span className="fox-contact-row-name">{item.displayName}</span>
              {item.pinnedAt && <Pin size={11} aria-label="已置顶" />}
            </button>
            <span className="fox-contact-row-status">
              {item.pendingActionCount > 0 && <small className="fox-contact-flag is-action" aria-label="需要你确认">需要你确认</small>}
              {item.activeRunStatus && <small className="fox-contact-flag is-working">正在工作</small>}
              {item.unreadCount > 0 && <small className="fox-contact-flag is-unread" aria-label={item.unreadCount + ' 条新消息'}>{item.unreadCount}</small>}
            </span>
            <DropdownMenu>
              <DropdownMenuTrigger asChild><Button type="button" variant="ghost" size="icon-sm" className="fox-icon-button fox-contact-row-menu" aria-label={'更多 ' + item.displayName + ' 操作'}><MoreHorizontal size={14} /></Button></DropdownMenuTrigger>
              <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-conversation-menu">
                <DropdownMenuItem onSelect={() => void open(item, true)}><UserRound />联系人资料</DropdownMenuItem>
                <DropdownMenuItem onSelect={() => void contacts.pin(item.id, !item.pinnedAt).then(refresh).catch(showContactError)}>{item.pinnedAt ? <PinOff /> : <Pin />}{item.pinnedAt ? '取消置顶' : '置顶'}</DropdownMenuItem>
                <DropdownMenuItem onSelect={() => void open(item, true)}><Pencil />重命名</DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem disabled={Boolean(item.activeRunStatus)} onSelect={() => void archive(item)}><Archive />归档</DropdownMenuItem>
                {!item.isDefault && <DropdownMenuItem variant="destructive" disabled={Boolean(item.activeRunStatus)} onSelect={() => runAfterMenuClose(() => setDeleting(item))}><Trash2 />删除…</DropdownMenuItem>}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
          {preview && <span id={previewId} className="fox-contact-row-preview">{preview}</span>}
        </div>
      </div>
    })}
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
  const searchRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const returnFocus = useRef(false)
  const { mode, close } = search
  useEffect(() => {
    if (mode !== 'panel') return
    const dismiss = (event: PointerEvent) => {
      const target = event.target
      if (target instanceof Element && !searchRef.current?.contains(target) && !target.closest('.fox-contact-search-range-menu')) close()
    }
    document.addEventListener('pointerdown', dismiss, true)
    return () => document.removeEventListener('pointerdown', dismiss, true)
  }, [mode, close])
  useEffect(() => {
    if (mode !== 'closed' || !returnFocus.current) return
    returnFocus.current = false
    triggerRef.current?.focus({ preventScroll: true })
  }, [mode])
  const closeFromSearch = () => { returnFocus.current = true; close() }
  return <header className="fox-chat-topbar fox-contact-topbar">
    <button type="button" className="fox-contact-heading" title={contactName} onClick={ctx?.showProfile}>
      {ctx?.current ? <ContactAvatar contact={ctx.current} /> : <UserRound size={17} />}<strong>{contactName}</strong><ChevronRight size={14} className="fox-contact-heading-chevron" aria-hidden="true" />
    </button>
    <div className="fox-contact-search" data-mode={mode} ref={searchRef} onKeyDown={event => {
      if (event.key === 'Escape' && !event.defaultPrevented && !event.nativeEvent.isComposing && mode !== 'closed') {
        event.preventDefault()
        event.stopPropagation()
        closeFromSearch()
      }
    }}>
      {mode === 'closed' && <Button ref={triggerRef} type="button" variant="ghost" size="icon" className="fox-contact-search-trigger" title="搜索私聊" aria-label="搜索私聊" onClick={search.open}><Search className="size-[18px]" strokeWidth={1.75} aria-hidden="true" /></Button>}
      {mode === 'panel' && <>
        <ContactSearchBar search={search} onClose={closeFromSearch} />
        <ContactSearchPanel search={search} contact={ctx?.current} contactName={contactName} />
      </>}
      {(mode === 'locating' || mode === 'chip') && <ContactSearchChip search={search} onClose={closeFromSearch} />}
    </div>
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
