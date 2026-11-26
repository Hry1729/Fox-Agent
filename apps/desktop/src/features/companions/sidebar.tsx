import { SidebarStatusDot } from '@/features/chat/sidebar-status-dot'
import { useEffect, useState, type KeyboardEvent } from 'react'
import { DndContext, PointerSensor, closestCenter, useDraggable, useDroppable, useSensor, useSensors, type DragEndEvent } from '@dnd-kit/core'
import { BellOff, Archive, Check, ChevronDown, GripVertical, Mail, MailOpen, MoreHorizontal, Pencil, Pin, PinOff, Plus, RotateCcw, Search, Star, StarOff, Trash2, UserRound, Users } from 'lucide-react'
import { AnimatedIconTrigger, Archive as AnimatedArchive, ArrowLeft, CalendarClock, Inbox, UserRoundPlus, Users as AnimatedUsers } from '@/components/fox-ui/animated-icons'
import { ContextMenu as RadixContextMenu } from 'radix-ui'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from '@/components/ui/context-menu'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import type { NavigateWorkspace, WorkspaceView } from '@/features/workspace/types'
import { asAgentCard } from '@/features/agents/agent-card-data'
import { defaultProfileAvatars } from '@/features/profile/user-profile'
import { CatPaw } from '@/components/animate-ui/icons/cat-paw'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { AgentRecord } from '@/features/conversations/model/types'
import { contacts, type Companion } from './api'
import { applyCompanionDefaults } from './defaults'
import { notify as toast } from '@/features/notifications'
import { useContacts, showContactError } from './context'
import { ContactAvatar } from './chat-controls'
import { ContactDelete } from './delete'
import { formatSearchTime } from './chat-search'
import './contacts.css'

function CompanionStatus({ contact, large = false }: { contact: Companion; large?: boolean }) {
  const status = contact.pendingActionCount > 0 ? 'action' : contact.activeRunStatus ? 'running' : contact.unreadCount > 0 ? 'unread' : null
  if (!status) return null
  const label = status === 'action' ? '等待你确认' : status === 'running' ? '正在工作' : `${contact.unreadCount} 条未读消息`
  return <span className={`fox-companion-status is-${status} ${large ? 'is-large' : ''} ${contact.reminderMode === 'muted' ? 'is-muted' : ''}`} role="img" aria-label={label}>{status === 'action' ? '!' : ''}</span>
}

function CreateCompanionDialog({ open, onOpenChange, creating, onCreate }: {
  open: boolean
  onOpenChange: (open: boolean) => void
  creating: boolean
  onCreate: (expertId: string) => Promise<void>
}) {
  const [experts, setExperts] = useState<AgentRecord[]>([])
  const [selected, setSelected] = useState('')
  const [query, setQuery] = useState('')
  const [category, setCategory] = useState('all')
  const categories = [...new Set(experts.map(agent => agent.category).filter(Boolean))]
  const choices = experts.filter(agent => (category === 'all' || agent.category === category)
    && `${agent.name} ${agent.description ?? ''}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
  const selectedExpert = experts.find(agent => agent.id === selected)

  const [loading, setLoading] = useState(false)
  const load = async () => {
    setLoading(true)
    try {
      const items = await desktopClient.listAgents()
      setExperts(items.filter(item => item.available && item.runtimeType === 'pi' && item.agentKind === 'expert'))
    } catch (error) { showContactError(error) } finally { setLoading(false) }
  }
  return <Dialog open={open} onOpenChange={next => { if (!creating) onOpenChange(next) }}>
    <DialogContent className="fox-companion-modal fox-companion-expert-picker" aria-describedby={undefined}
      onOpenAutoFocus={() => { setQuery(''); setCategory('all'); setSelected(''); void load() }}>
      <DialogHeader><DialogTitle>从专家创建</DialogTitle></DialogHeader>
      <div className="fox-companion-picker-tools">
        <label className="fox-expert-picker-search"><Search size={16} aria-hidden /><Input type="search" aria-label="搜索专家" placeholder="搜索专家名称或用处" value={query} onChange={event => setQuery(event.target.value)} /></label>
        <Select value={category} onValueChange={setCategory}><SelectTrigger aria-label="专家分类"><SelectValue /></SelectTrigger><SelectContent>
          <SelectItem value="all">全部分类</SelectItem>{categories.map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}
        </SelectContent></Select>
      </div>
      <div className="fox-companion-expert-choices" aria-label="可用专家" aria-busy={loading}>
        {loading ? <p>正在加载专家…</p> : choices.map(agent => <button type="button" className="fox-companion-expert-choice" key={agent.id}
          aria-label={`选择${agent.name}`} aria-pressed={selected === agent.id} disabled={creating} onClick={() => setSelected(agent.id)}>
          <img src={asAgentCard(agent).image} alt="" />
          <span className="fox-companion-expert-copy">
            <span className="fox-companion-expert-name"><strong>{agent.name}</strong>
              {selected === agent.id && <span className="fox-companion-picker-check"><Check size={11} aria-hidden /></span>}
            </span>
            <small>{agent.description}</small>
          </span>
        </button>)}
        {!loading && !choices.length && <p>{query || category !== 'all' ? '没有找到匹配的专家' : '暂无可用专家'}</p>}
      </div>
      <DialogFooter>
        <span className="fox-companion-picked-expert">{selectedExpert
          ? <><img src={asAgentCard(selectedExpert).image} alt="" /><span>已选 <strong>{selectedExpert.name}</strong></span></>
          : '请选择一位专家'}</span>
        <Button variant="outline" disabled={creating} onClick={() => onOpenChange(false)}>取消</Button>
        <Button disabled={creating || !selected} onClick={() => void onCreate(selected)}>{creating ? '正在创建…' : '创建'}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

function useCompanionActions(openCompanionPage?: (contact: Companion) => void) {
  const ctx = useContacts()
  const [creating, setCreating] = useState(false)
  const [expertOpen, setExpertOpen] = useState(false)
  const [blankOpen, setBlankOpen] = useState(false)
  const [deleting, setDeleting] = useState<Companion | null>(null)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [name, setName] = useState('')
  if (!ctx) return null
  const open = async (contact: Companion, profile = false) => {
    try { ctx.open(contact.archivedAt ? contact : await contacts.open(contact.id), profile) } catch (error) { showContactError(error) }
  }
  const openInCompanionPage = async (contact: Companion) => {
    try {
      const opened = await contacts.open(contact.id)
      if (openCompanionPage) openCompanionPage(opened)
      else ctx.open(opened)
    } catch (error) { showContactError(error) }
  }
  const create = async (expertId?: string, profile?: { displayName: string; avatarRef: string; duty?: string }) => {
    if (creating) return
    setCreating(true)
    try {
      const created = expertId ? await contacts.add(expertId) : await contacts.createBlank(profile && { displayName: profile.displayName, avatarRef: profile.avatarRef })
      const initialized = await applyCompanionDefaults(created)
      let contact = initialized.contact
      if (profile?.duty) {
        try { contact = await contacts.profile(created.id, { duty: profile.duty }) }
        catch (error) { toast.error('伙伴已创建，职责保存失败', { description: desktopErrorDetails(error).message }) }
      }
      ctx.open(contact)
      if (initialized.errors.length) toast.error('伙伴已创建，部分默认值未应用', { description: initialized.errors.join('；') })
      setExpertOpen(false)
      setBlankOpen(false)
      await ctx.refresh().catch(showContactError)
    } catch (error) { if (profile) throw error; showContactError(error) } finally { setCreating(false) }
  }
  const archive = async (contact: Companion) => {
    try { await contacts.archive(contact.id); ctx.leave(contact); await ctx.refresh() } catch (error) { showContactError(error) }
  }
  const restore = async (contact: Companion) => {
    try { await contacts.restore(contact.id); await ctx.refresh() } catch (error) { showContactError(error) }
  }
  const toggleWork = async (contact: Companion) => {
    const selected = ctx.list.companions.filter(item => item.workShortcutOrder != null && !item.archivedAt && !item.deletingAt).sort((a, b) => (a.workShortcutOrder ?? 0) - (b.workShortcutOrder ?? 0))
    const ids = selected.some(item => item.id === contact.id) ? selected.filter(item => item.id !== contact.id).map(item => item.id) : [...selected.map(item => item.id), contact.id]
    try { await contacts.setWorkShortcuts(ids); await ctx.refresh() } catch (error) { showContactError(error) }
  }
  const rename = async (contact: Companion) => {
    const displayName = name.trim()
    setRenaming(null)
    if (!displayName || displayName === contact.displayName) return
    try { await contacts.profile(contact.id, { displayName }); await ctx.refresh() } catch (error) { showContactError(error) }
  }
  const changeRead = async (contact: Companion) => {
    try {
      if (contact.unreadCount > 0 && contact.latestDeliveryOrder != null) {
        await contacts.markRead(contact.id, contact.latestDeliveryOrder, Date.now())
      } else if (contact.latestBubbleId) {
        await contacts.markUnread(contact.id, contact.latestBubbleId)
      }
      await ctx.refresh()
    } catch (error) { showContactError(error) }
  }
  return { ctx, creating, expertOpen, setExpertOpen, blankOpen, setBlankOpen, deleting, setDeleting, renaming, setRenaming, name, setName, open, openInCompanionPage, create, archive, restore, toggleWork, rename, changeRead }
}

type Actions = NonNullable<ReturnType<typeof useCompanionActions>>

const companionTemplates = [
  { label: '日常助理', name: '小狐助理', duty: '梳理待办、跟进日程并起草常用消息。' },
  { label: '写作搭档', name: '文字搭档', duty: '协助编写和修改周报、产品文案与汇报材料。' },
  { label: '调研助手', name: '情报小狐', duty: '查找并核验一手资料，记录调研结果。' },
  { label: '技术顾问', name: '代码搭档', duty: '审查代码改动、排查问题并维护技术决策记录。' },
]

function BlankCompanionDialog({ actions }: { actions: Actions }) {
  const [name, setName] = useState('')
  const [template, setTemplate] = useState<(typeof companionTemplates)[number]>()
  const [avatarRef, setAvatarRef] = useState(defaultProfileAvatars[0] ?? '/mascot/fox_magic.png')
  const [error, setError] = useState('')
  useEffect(() => {
    let active = true
    void contacts.nextDefaultName().then(suggestion => { if (active) setName(current => current || suggestion) })
      .catch(cause => { if (active) setError(desktopErrorDetails(cause).message) })
    return () => { active = false }
  }, [])
  return <Dialog open onOpenChange={open => { if (!open && !actions.creating) actions.setBlankOpen(false) }}>
    <DialogContent className="fox-companion-modal fox-blank-companion-dialog" aria-describedby={undefined}>
      <DialogHeader><DialogTitle>新建伙伴</DialogTitle></DialogHeader>
      <form className="fox-blank-companion-form" onSubmit={event => {
        event.preventDefault()
        if (!name.trim() || actions.creating) return
        setError('')
        void actions.create(undefined, { displayName: name.trim(), avatarRef, duty: template?.duty }).catch(cause => setError(desktopErrorDetails(cause).message))
      }}>
        <div className="fox-companion-avatar-preview"><img src={avatarRef} alt="伙伴头像预览" /></div>
        <Input className="fox-companion-name-input" aria-label="伙伴名称" placeholder="给你的伙伴起个名字" value={name} maxLength={40}
          autoFocus disabled={actions.creating} onChange={event => setName(event.currentTarget.value)} />
        <fieldset><legend className="sr-only">选择头像</legend><div className="fox-avatar-picker">
          {defaultProfileAvatars.map((src, index) => <label key={src} className={avatarRef === src ? 'is-selected' : undefined}>
            <input type="radio" name="companion-avatar" aria-label={`头像 ${index + 1}`} checked={avatarRef === src}
              disabled={actions.creating} onChange={() => setAvatarRef(src)} />
            <img src={src} alt="" />{avatarRef === src && <Check size={14} aria-hidden />}
          </label>)}
        </div></fieldset>
        <fieldset><legend className="sr-only">快捷角色</legend><div className="fox-companion-modal-pills">
          {companionTemplates.map(item => <Button key={item.label} type="button" variant={template === item ? 'secondary' : 'outline'} size="sm"
            aria-pressed={template === item} disabled={actions.creating} onClick={() => { setTemplate(item); setName(item.name) }}>{item.label}</Button>)}
        </div><p className="fox-companion-preset-hint">{template?.duty ?? '点选快捷角色可快速起名'}</p></fieldset>
        {error && <p className="fox-contact-decision-error" role="alert">{error}</p>}
        <DialogFooter>
          <Button type="button" variant="outline" disabled={actions.creating} onClick={() => actions.setBlankOpen(false)}>取消</Button>
          <Button type="submit" disabled={actions.creating || !name.trim()}>{actions.creating ? '正在创建…' : '创建'}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
}

export function NewCompanionButton() {
  const actions = useCompanionActions()
  if (!actions) return null
  return <>
    <Button onClick={() => actions.setBlankOpen(true)}><Plus />新建伙伴</Button>
    {actions.blankOpen && <BlankCompanionDialog actions={actions} />}
  </>
}

function CompanionCreateMenu({ actions, collapsed = false }: { actions: Actions; collapsed?: boolean }) {
  const label = actions.creating ? '正在创建…' : '新建伙伴'
  return <>
    <DropdownMenu><AnimatedIconTrigger><DropdownMenuTrigger asChild>
      <button type="button" className={`fox-sidebar-command fox-companion-create${collapsed ? '' : ' is-emphasis'}`}
        aria-label={label} title={collapsed ? label : undefined} disabled={actions.creating}>
        <UserRoundPlus size={18} aria-hidden />
        {!collapsed && <><span>{label}</span><ChevronDown size={14} aria-hidden /></>}
      </button>
    </DropdownMenuTrigger></AnimatedIconTrigger><DropdownMenuContent align="start" side={collapsed ? 'right' : 'bottom'} className="fox-conversation-menu fox-companion-create-menu">
      <AnimatedIconTrigger><DropdownMenuItem aria-label="新建伙伴" disabled={actions.creating} onSelect={() => runAfterMenuClose(() => actions.setBlankOpen(true))}>
        <UserRoundPlus /><span>新建伙伴</span>
      </DropdownMenuItem></AnimatedIconTrigger>
      <AnimatedIconTrigger><DropdownMenuItem aria-label="从专家创建" disabled={actions.creating} onSelect={() => runAfterMenuClose(() => actions.setExpertOpen(true))}>
        <AnimatedUsers /><span>从专家创建</span>
      </DropdownMenuItem></AnimatedIconTrigger>
    </DropdownMenuContent></DropdownMenu>
    <CompanionCreateDialogs actions={actions} />
  </>
}

function CompanionCreateDialogs({ actions }: { actions: Actions }) {
  return <>
    {actions.expertOpen && <CreateCompanionDialog open={actions.expertOpen} onOpenChange={actions.setExpertOpen} creating={actions.creating} onCreate={actions.create} />}
    {actions.blankOpen && <BlankCompanionDialog actions={actions} />}
  </>
}

function CompanionMenu({ contact, actions, archived = false, work = false, context = false }: { contact: Companion; actions: Actions; archived?: boolean; work?: boolean; context?: boolean }) {
  const { ctx } = actions
  const Item = context ? ContextMenuItem : DropdownMenuItem
  const Separator = context ? ContextMenuSeparator : DropdownMenuSeparator
  const inWork = contact.isDefault || contact.workShortcutOrder != null
  if (archived) return <Item variant="destructive" onSelect={() => runAfterMenuClose(() => actions.setDeleting(contact))}><Trash2 />删除…</Item>
  if (work) return <>
    <Item onSelect={() => void actions.open(contact, true)}><UserRound />伙伴资料</Item>
    <Item onSelect={() => void actions.openInCompanionPage(contact)}><CatPaw />在伙伴页面中查看</Item>
    {!contact.isDefault && <><Separator /><Item onSelect={() => void actions.toggleWork(contact)}><StarOff />从常用中移除</Item></>}
  </>
  return <>
    <Item onSelect={() => void actions.open(contact, true)}><UserRound />伙伴资料</Item>
    {context ? <RadixContextMenu.CheckboxItem checked={inWork} disabled={contact.isDefault} onCheckedChange={() => void actions.toggleWork(contact)}
      className="fox-companion-context-checkbox"><Star />显示在工作页{inWork && <Check />}</RadixContextMenu.CheckboxItem>
      : <DropdownMenuCheckboxItem checked={inWork} disabled={contact.isDefault} onCheckedChange={() => void actions.toggleWork(contact)}>显示在工作页</DropdownMenuCheckboxItem>}
    {!contact.isDefault && <Item onSelect={() => void contacts.pin(contact.id, !contact.pinnedAt).then(ctx.refresh).catch(showContactError)}>{contact.pinnedAt ? <PinOff /> : <Pin />}{contact.pinnedAt ? '取消置顶' : '置顶'}</Item>}
    <Item onSelect={() => runAfterMenuClose(() => { actions.setName(contact.displayName); actions.setRenaming(contact.id) })}><Pencil />重命名</Item>
    <Item disabled={contact.unreadCount > 0 ? contact.latestDeliveryOrder == null : !contact.latestBubbleId} onSelect={() => void actions.changeRead(contact)}>
      {contact.unreadCount > 0 ? <><MailOpen />标记为已读</> : <><Mail />标记为未读</>}
    </Item>
    {!contact.isDefault && <>
      <Separator />
      <Item disabled={Boolean(contact.activeRunStatus)} onSelect={() => void actions.archive(contact)}><Archive />归档</Item>
      <Item variant="destructive" disabled={Boolean(contact.activeRunStatus)} onSelect={() => runAfterMenuClose(() => actions.setDeleting(contact))}><Trash2 />删除…</Item>
    </>}
  </>
}

function NameInput({ contact, actions }: { contact: Companion; actions: Actions }) {
  return <input className="fox-companion-rename" aria-label={`重命名 ${contact.displayName}`} autoFocus maxLength={40} value={actions.name}
    onFocus={event => event.currentTarget.select()} onChange={event => actions.setName(event.currentTarget.value)}
    onBlur={() => void actions.rename(contact)} onKeyDown={event => {
      if (event.key === 'Enter' && !event.nativeEvent.isComposing) { event.preventDefault(); event.currentTarget.blur() }
      if (event.key === 'Escape') { event.preventDefault(); actions.setRenaming(null) }
    }} />
}

function openContextWithKeyboard(event: KeyboardEvent<HTMLButtonElement>) {
  if (event.key !== 'ContextMenu' && !(event.shiftKey && event.key === 'F10')) return
  event.preventDefault()
  const box = event.currentTarget.getBoundingClientRect()
  event.currentTarget.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: box.left + 10, clientY: box.bottom }))
}

function CompanionRow({ contact, actions, activeId, archived = false, work = false }: { contact: Companion; actions: Actions; activeId?: string; archived?: boolean; work?: boolean }) {
  const editing = actions.renaming === contact.id
  const preview = contact.latestMessagePreview?.replace(/\s+/g, ' ').trim() || contact.duty || '还没有固定职责'
  const waiting = !archived && contact.pendingActionCount > 0
  const failure = !archived && !waiting && !contact.activeRunStatus ? contact.latestFailureSummary : null
  const summary = archived && contact.archivedAt != null ? `归档于 ${formatSearchTime(contact.archivedAt)}`
    : waiting ? `等待你确认：${preview}`
      : contact.activeRunStatus ? `正在处理：${preview}` : preview
  return <ContextMenu><ContextMenuTrigger asChild disabled={editing}><div className={`fox-contact-row fox-companion-row ${archived ? 'is-archived' : ''} ${activeId === contact.conversationId ? 'is-active' : ''}`} data-sidebar-motion-item data-highlight="true">
    <span className="fox-companion-row-avatar"><ContactAvatar contact={contact} /></span>
    <div className="fox-contact-row-copy">
        <div className="fox-contact-row-head">{editing ? <NameInput contact={contact} actions={actions} /> : <button type="button" className="fox-contact-link" onKeyDown={openContextWithKeyboard} onClick={() => void actions.open(contact)} aria-current={activeId === contact.conversationId ? 'page' : undefined}><span className="fox-contact-row-name">{contact.displayName}</span>{contact.reminderMode === 'muted' && <BellOff size={12} aria-label="免打扰" />}{contact.pinnedAt && !archived && <Pin size={11} aria-label="已置顶" />}</button>}
          {!work && !archived && contact.lastMessageAt != null && <time className="fox-companion-row-time" dateTime={new Date(contact.lastMessageAt).toISOString()}>{formatSearchTime(contact.lastMessageAt)}</time>}
        </div>
        <div className="fox-companion-preview-line"><span className={`fox-contact-row-preview ${waiting ? 'is-waiting' : ''}`} title={summary}>{summary}</span></div>
    </div>
    {!archived && <SidebarStatusDot state={failure ? 'error' : waiting ? 'approval' : contact.unreadCount > 0 ? 'unread' : undefined} label={failure ? '运行异常' : waiting ? '等待你确认' : `${contact.unreadCount} 条未读`} />}
    {archived && <Button type="button" variant="ghost" size="icon-sm" className="fox-icon-button fox-companion-row-restore" aria-label={`恢复 ${contact.displayName}`} title="恢复伙伴" onClick={() => void actions.restore(contact)}><RotateCcw size={14} /></Button>}
    {!editing && <DropdownMenu><DropdownMenuTrigger asChild><Button type="button" variant="ghost" size="icon-sm" className="fox-icon-button fox-contact-row-menu" aria-label={`更多 ${contact.displayName} 操作`}><MoreHorizontal size={14} /></Button></DropdownMenuTrigger><DropdownMenuContent align="end" side="bottom" sideOffset={4} alignOffset={-5} className="fox-conversation-menu fox-companion-row-menu-panel"><CompanionMenu contact={contact} actions={actions} archived={archived} work={work} /></DropdownMenuContent></DropdownMenu>}
  </div></ContextMenuTrigger><ContextMenuContent className="fox-conversation-menu fox-companion-row-menu-panel"><CompanionMenu contact={contact} actions={actions} archived={archived} work={work} context /></ContextMenuContent></ContextMenu>
}

function CompanionTile({ contact, actions, activeId, onMove, sorting }: { contact: Companion; actions: Actions; activeId?: string; onMove: (id: string, offset: number) => void; sorting: boolean }) {
  const editableOrder = !contact.isDefault
  const { attributes, listeners, setNodeRef: dragRef, setActivatorNodeRef, transform, isDragging } = useDraggable({ id: contact.id, disabled: !editableOrder || sorting })
  const { setNodeRef: dropRef } = useDroppable({ id: contact.id, disabled: !editableOrder || sorting })
  if (actions.renaming === contact.id) return <div className="fox-companion-tile fox-companion-tile-edit">
    <span className="fox-companion-tile-avatar"><ContactAvatar contact={contact} /></span>
    <NameInput contact={contact} actions={actions} />
  </div>
  const statusLabel = contact.pendingActionCount > 0 ? '，等待你确认' : contact.activeRunStatus ? '，正在工作' : contact.unreadCount > 0 ? `，${contact.unreadCount} 条未读消息` : ''
  return <div ref={node => { dragRef(node); dropRef(node) }} className={`fox-companion-tile ${activeId === contact.conversationId ? 'is-active' : ''}`}
    data-sidebar-motion-item data-highlight="true" data-dragging={isDragging || undefined}
    style={transform ? { transform: `translate3d(${transform.x}px,${transform.y}px,0)` } : undefined}>
    <ContextMenu><ContextMenuTrigger asChild><button type="button" className="fox-companion-tile-link" title={contact.displayName} aria-label={`${contact.displayName}${statusLabel}`} onKeyDown={openContextWithKeyboard} onClick={() => void actions.open(contact)} aria-current={activeId === contact.conversationId ? 'page' : undefined}><span className="fox-companion-tile-avatar"><ContactAvatar contact={contact} /><CompanionStatus contact={contact} large /></span><span>{contact.displayName}</span></button></ContextMenuTrigger><ContextMenuContent className="fox-conversation-menu fox-companion-row-menu-panel"><CompanionMenu contact={contact} actions={actions} context /></ContextMenuContent></ContextMenu>
    {editableOrder && <button type="button" ref={setActivatorNodeRef} {...attributes} {...listeners} className="fox-companion-tile-grip"
      disabled={sorting} aria-label={`调整 ${contact.displayName} 的置顶顺序`} title="拖动或使用方向键调整置顶顺序"
      onKeyDown={event => {
        const offset = event.key === 'ArrowLeft' ? -1 : event.key === 'ArrowRight' ? 1 : event.key === 'ArrowUp' ? -3 : event.key === 'ArrowDown' ? 3 : 0
        if (offset) { event.preventDefault(); onMove(contact.id, offset) }
      }}><GripVertical size={13} aria-hidden /></button>}
  </div>
}

function moveItem(ids: string[], active: string, over: string) {
  if (active === over || !ids.includes(active) || !ids.includes(over)) return ids
  const next = [...ids]
  const overIndex = next.indexOf(over)
  next.splice(next.indexOf(active), 1)
  next.splice(overIndex, 0, active)
  return next
}

function WorkShortcutRow({ contact, onMove, onRemove }: { contact: Companion; onMove: (direction: -1 | 1) => void; onRemove: () => void }) {
  const { attributes, listeners, setNodeRef: dragRef, setActivatorNodeRef, transform, isDragging } = useDraggable({ id: contact.id })
  const { setNodeRef: dropRef } = useDroppable({ id: contact.id })
  return <div ref={node => { dragRef(node); dropRef(node) }} className="fox-work-shortcut-row" data-dragging={isDragging || undefined} style={{ transform: transform ? `translate3d(${transform.x}px, ${transform.y}px, 0)` : undefined }}>
    <button type="button" ref={setActivatorNodeRef} {...attributes} {...listeners} className="fox-work-shortcut-grip" aria-label={`调整 ${contact.displayName} 的顺序`} onKeyDown={(event: KeyboardEvent<HTMLButtonElement>) => { if (event.key === 'ArrowUp' || event.key === 'ArrowDown') { event.preventDefault(); onMove(event.key === 'ArrowUp' ? -1 : 1) } }}><GripVertical size={13} /></button>
    <ContactAvatar contact={contact} /><span>{contact.displayName}</span>
    <button type="button" className="fox-work-shortcut-action" aria-label={`从常用中移除 ${contact.displayName}`} onClick={onRemove}>移除</button>
  </div>
}

export function WorkCompanionSidebar({ activeId, collapsed = false, onOpenCompanions }: { activeId?: string; collapsed?: boolean; onOpenCompanions: (companionId?: string) => void }) {
  const actions = useCompanionActions(contact => onOpenCompanions(contact.id))
  const [managerOpen, setManagerOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [draftOrder, setDraftOrder] = useState<string[] | null>(null)
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }))
  if (!actions) return null
  const { ctx } = actions
  const available = ctx.list.companions.filter(item => !item.archivedAt && !item.deletingAt)
  const defaultContact = available.find(item => item.isDefault)
  const shortcuts = available.filter(item => item.workShortcutOrder).sort((a, b) => (a.workShortcutOrder ?? 0) - (b.workShortcutOrder ?? 0))
  const selectedIds = draftOrder ?? shortcuts.map(item => item.id)
  const selected = selectedIds.map(id => available.find(item => item.id === id)).filter((item): item is Companion => Boolean(item))
  const visible = [...(defaultContact ? [defaultContact] : []), ...shortcuts]
  if (collapsed) return <div className="fox-collapsed-companions" aria-label="常用伙伴">{visible.map(item => <button type="button" key={item.id} aria-label={item.displayName} title={item.displayName} onClick={() => void actions.open(item)}><ContactAvatar contact={item} /><CompanionStatus contact={item} /></button>)}</div>
  const candidates = available.filter(item => !item.isDefault && !selectedIds.includes(item.id) && `${item.displayName} ${item.duty}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
  const save = async (ids: string[]) => {
    setDraftOrder(ids)
    try { await contacts.setWorkShortcuts(ids); await ctx.refresh(); setDraftOrder(null) } catch (error) { setDraftOrder(null); showContactError(error) }
  }
  const move = (id: string, direction: -1 | 1) => {
    const index = selectedIds.indexOf(id)
    const over = selectedIds[index + direction]
    if (over) void save(moveItem(selectedIds, id, over))
  }
  const dragEnd = ({ active, over }: DragEndEvent) => {
    if (over) void save(moveItem(selectedIds, String(active.id), String(over.id)))
  }
  return <section className="fox-contact-sidebar fox-work-companions" aria-label="伙伴">
    <div className="fox-section-head fox-sidebar-group-head"><span>伙伴</span><Popover open={managerOpen} onOpenChange={open => { setManagerOpen(open); if (!open) setDraftOrder(null) }}><PopoverTrigger asChild><Button type="button" variant="ghost" size="icon" className="fox-icon-button" aria-label="管理常用伙伴"><MoreHorizontal size={14} /></Button></PopoverTrigger><PopoverContent side="right" sideOffset={16} align="start" alignOffset={-8} className="fox-work-shortcut-manager">
      <div className="fox-work-shortcut-head"><strong>常用伙伴</strong><span>已选 {selectedIds.length} / 3</span></div>
      <label className="fox-work-shortcut-search"><Search size={14} /><input value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索伙伴" /></label>
      <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={dragEnd}><div className="fox-work-shortcut-list" aria-label="已选常用伙伴">
        {defaultContact && <div className="fox-work-shortcut-row is-default"><span className="fox-work-shortcut-grip" /><ContactAvatar contact={defaultContact} /><span>{defaultContact.displayName}</span><small>默认</small></div>}
        {selected.map(item => <WorkShortcutRow key={item.id} contact={item} onMove={direction => move(item.id, direction)} onRemove={() => void save(selectedIds.filter(id => id !== item.id))} />)}
      </div></DndContext>
      {candidates.length > 0 && <div className="fox-work-shortcut-label">其他伙伴</div>}
      <div className="fox-work-shortcut-list" aria-label="其他伙伴">{candidates.map(item => <div className="fox-work-shortcut-row" key={item.id}><span className="fox-work-shortcut-grip" /><ContactAvatar contact={item} /><span>{item.displayName}</span><button type="button" className="fox-work-shortcut-action is-add" aria-label={`添加 ${item.displayName}`} title={selectedIds.length >= 3 ? '最多 3 个常用伙伴' : `添加 ${item.displayName}`} disabled={selectedIds.length >= 3} onClick={() => void save([...selectedIds, item.id])}><Plus size={13} />添加</button></div>)}</div>
      {selectedIds.length >= 3 && <p className="fox-work-shortcut-hint">最多选择 3 个常用伙伴，移除后可继续添加。</p>}
    </PopoverContent></Popover></div>
    {ctx.error && <button type="button" title={ctx.error} onClick={() => void ctx.refresh()}>重新加载伙伴</button>}
    {visible.map(item => <CompanionRow key={item.id} contact={item} actions={actions} activeId={activeId} work />)}
    {actions.deleting && <ContactDelete contact={actions.deleting} close={() => actions.setDeleting(null)} />}
  </section>
}

export function CompanionSidebar({ activeId, collapsed = false, scroll = false, view, navigate, enabledTaskCount }: {
  activeId?: string; collapsed?: boolean; scroll?: boolean
  view?: WorkspaceView; navigate?: NavigateWorkspace; enabledTaskCount?: number
}) {
  const actions = useCompanionActions()
  const [archivedOpen, setArchivedOpen] = useState(false)
  const [draftPinned, setDraftPinned] = useState<string[] | null>(null)
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }))
  if (!actions) return null
  const { ctx } = actions
  const active = ctx.list.companions.filter(item => !item.archivedAt && !item.deletingAt)
  const archived = ctx.list.companions.filter(item => item.archivedAt && !item.deletingAt)
    .sort((a, b) => (b.archivedAt ?? 0) - (a.archivedAt ?? 0))
  const defaultContact = active.find(item => item.isDefault)
  const persistedPinned = active.filter(item => !item.isDefault && item.pinnedAt != null)
    .sort((a, b) => (a.pinnedOrder ?? -Infinity) - (b.pinnedOrder ?? -Infinity) || (b.pinnedAt ?? 0) - (a.pinnedAt ?? 0))
  const pinnedIds = draftPinned ?? persistedPinned.map(item => item.id)
  const pinned = [...(defaultContact ? [defaultContact] : []), ...pinnedIds.flatMap(id => persistedPinned.find(item => item.id === id) ?? [])]
  const ordinary = active.filter(item => !item.isDefault && !item.pinnedAt)
  const saveOrder = async (id: string, over: string) => {
    const next = moveItem(pinnedIds, id, over)
    if (next === pinnedIds || draftPinned) return
    setDraftPinned(next)
    try { await contacts.setPinnedOrder(next); await ctx.refresh() } catch (error) { showContactError(error) } finally { setDraftPinned(null) }
  }
  const movePinned = (id: string, offset: number) => {
    const target = pinnedIds[pinnedIds.indexOf(id) + offset]
    if (target) void saveOrder(id, target)
  }
  const dragEnd = ({ active: dragged, over }: DragEndEvent) => {
    if (over) void saveOrder(String(dragged.id), String(over.id))
  }
  const destinations = [
    { id: 'companion-inbox', label: '待我处理', Icon: Inbox, count: active.reduce((total, item) => total + item.pendingActionCount, 0) },
    { id: 'companion-schedules', label: '定时任务', Icon: CalendarClock, count: enabledTaskCount },
    { id: 'companion-overview', label: '全部伙伴', Icon: AnimatedUsers, count: undefined },
  ] as const
  const globalNav = navigate && <nav className="fox-companion-global-nav" aria-label="伙伴工作区">
    {destinations.map(({ id, label, Icon, count }) => <AnimatedIconTrigger key={id}><button type="button" className={`fox-sidebar-command ${view === id ? 'is-active' : ''}`}
      aria-current={view === id ? 'page' : undefined} aria-label={count ? `${label}，${count} 项` : label} title={label}
      onClick={() => { setArchivedOpen(false); navigate(id) }}><Icon size={16} aria-hidden />{!collapsed && <span>{label}</span>}{Boolean(count) && <small>{count}</small>}</button></AnimatedIconTrigger>)}
  </nav>
  const primary = <div className="fox-sidebar-primary fox-companion-primary">
    <CompanionCreateMenu actions={actions} collapsed={collapsed} />
    {globalNav}
  </div>
  const archiveRows = archived.map(item => <CompanionRow key={item.id} contact={item} actions={actions} activeId={activeId} archived />)
  if (collapsed) return <div className="fox-collapsed-companion-page">
    {primary}
    <div className="fox-collapsed-companions" aria-label="伙伴">{[...pinned, ...ordinary].map(item => <button type="button" key={item.id}
      aria-label={item.displayName} title={item.displayName} aria-current={activeId === item.conversationId ? 'page' : undefined}
      onClick={() => void actions.open(item)}><ContactAvatar contact={item} /><CompanionStatus contact={item} /></button>)}</div>
  </div>
  if (archivedOpen && archived.length > 0) return <section className={`fox-contact-sidebar fox-companion-page ${scroll ? 'fox-companion-sidebar-frame' : ''}`} aria-label="已归档伙伴">
    <header className="fox-companion-archive-head">
      <AnimatedIconTrigger><Button type="button" variant="ghost" size="icon-sm" aria-label="返回伙伴列表" onClick={() => setArchivedOpen(false)}><ArrowLeft size={15} aria-hidden /></Button></AnimatedIconTrigger>
      <strong>已归档</strong><span>{archived.length}</span>
    </header>
    {scroll ? <ScrollArea className="fox-sidebar-scroll fox-sidebar-group-scroll">{archiveRows}</ScrollArea> : archiveRows}
    {actions.deleting && <ContactDelete contact={actions.deleting} close={() => actions.setDeleting(null)} />}
  </section>
  const listing = <>
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={dragEnd}>
      <div className="fox-companion-tile-grid" aria-label="置顶伙伴">{pinned.map(item => <CompanionTile key={item.id} contact={item} actions={actions} activeId={activeId} onMove={movePinned} sorting={draftPinned != null} />)}</div>
    </DndContext>
    {ordinary.length > 0 && <section className="fox-companionordinary-list" aria-label="伙伴私聊">
      <header><span>全部私聊</span><span>{ordinary.length}</span></header>
      {ordinary.map(item => <CompanionRow key={item.id} contact={item} actions={actions} activeId={activeId} />)}
    </section>}
  </>
  return <section className={`fox-contact-sidebar fox-companion-page ${scroll ? 'fox-companion-sidebar-frame' : ''}`} aria-label="伙伴">
    {primary}
    {ctx.error && <button type="button" title={ctx.error} onClick={() => void ctx.refresh()}>重新加载伙伴</button>}
    {scroll ? <ScrollArea className="fox-sidebar-scroll fox-sidebar-group-scroll">{listing}</ScrollArea> : listing}
    {archived.length > 0 && <div className="fox-companion-archive"><AnimatedIconTrigger><button type="button" className="fox-companion-archive-toggle" onClick={() => setArchivedOpen(true)}><AnimatedArchive size={15} aria-hidden /><span>已归档</span><span>{archived.length}</span></button></AnimatedIconTrigger></div>}
    {actions.deleting && <ContactDelete contact={actions.deleting} close={() => actions.setDeleting(null)} />}
  </section>
}

export function CompanionOverview({ navigate, permissions }: { navigate: NavigateWorkspace; permissions: ReadonlyMap<string, string> }) {
  const actions = useCompanionActions()
  const [saving, setSaving] = useState(false)
  const active = actions?.ctx.list.companions.filter(item => !item.archivedAt && !item.deletingAt) ?? []
  return <div className="fox-companion-workspace-inner">
    <header className="fox-companion-workspace-head">
      <div><h1>全部伙伴</h1><p>查看职责、模型和权限，管理工作页快捷入口。</p></div>
      {actions && <div><Button variant="outline" onClick={() => actions.setExpertOpen(true)}><Users />从专家创建</Button><Button onClick={() => actions.setBlankOpen(true)}><Plus />新建伙伴</Button></div>}
    </header>
    {!active.length && <p className="fox-companion-workspace-empty">{actions?.ctx.error || '暂无常驻伙伴'}</p>}
    <div className="fox-companion-workspace-list">
      {active.map(item => <article key={item.id} className="fox-companion-workspace-row">
        <button type="button" className="fox-companion-workspace-person" onClick={() => void actions?.open(item)}>
          <ContactAvatar contact={item} /><span>
            <span className="fox-companion-workspace-row-title">
              <strong>{item.displayName}</strong>
              {item.isDefault && <small className="fox-companion-schedule-tag">默认伙伴</small>}
              {!item.isDefault && item.pinnedAt != null && <small className="fox-companion-schedule-tag">已置顶</small>}
            </span>
            <small>{item.duty}</small>
            <small>{[permissions.get(item.conversationId), item.modelRef || actions?.ctx.model].filter(Boolean).join(' · ')}</small>
          </span>
        </button>
        <label className="fox-companion-workspace-switch"><Switch checked={item.isDefault || item.workShortcutOrder != null}
          aria-label={`在工作页显示${item.displayName}`} disabled={item.isDefault || saving}
          onCheckedChange={() => {
            setSaving(true)
            void actions?.toggleWork(item).finally(() => setSaving(false))
          }} />工作页快捷栏</label>
        <Button size="sm" variant="ghost" onClick={() => void actions?.open(item)}>进入私聊</Button>
      </article>)}
    </div>
    <Button variant="ghost" onClick={() => navigate('settings-companions')}>设置 · 伙伴默认设置</Button>
    {actions && <CompanionCreateDialogs actions={actions} />}
  </div>
}
