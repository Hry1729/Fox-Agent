import { type ConversationRunIndicator } from '../conversation-run-indicator'
import { ConversationStatusIcon } from '../conversation-status-icon'
import { type SidebarProjectGroup, sidebarProjectGroups } from '../sidebar-project-groups'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from '@/components/ui/context-menu'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { type ConversationSummary, type GlobalSearchRecord, type ProjectRecord, type YuxiUserRecord } from '@/features/conversations/model/types'
import { LocalKnowledgeSidebarNavigation } from '@/features/local-knowledge/local-knowledge-sidebar'
import { notify as toast } from '@/features/notifications'
import { UserProfileDialog, useUserProfile } from '@/features/profile/user-profile'
import { SETTINGS_WORKSPACE_VIEWS } from '@/features/workspace/management-navigation'
import { type NavigateWorkspace, type WorkspaceView } from '@/features/workspace/types'
import { Activity, AlertTriangle, Archive, ArrowLeft, BookCopy, BookOpen, Bot, Brain, Cable, ChevronDown, ChevronRight, CircleHelp, ClipboardList, Database, FileEdit, Folder, FolderOpen, Folders, Globe2, House, Library, LoaderCircle, MessageCircleMore, MessageSquarePlus, MoreHorizontal, PackagePlus, Pin, PinOff, Plus, RotateCcw, Search, Server, Settings, ShieldCheck, Sparkles, SunMoon, Trash2, UserRound, UserRoundCheck, WandSparkles, Wrench, X } from 'lucide-react'
import { type KeyboardEvent, Suspense, lazy, memo, useCallback, useEffect, useRef, useState } from 'react'
import { type AssistantMode, IconButton, globalSearchKindLabel } from '../fox-shared'
import { NotificationCenter } from './fox-notification-center'
import { LOCAL_KNOWLEDGE_WORKSPACE_VIEWS } from '../workbench'

const KnowledgeResourceExplorer = lazy(() => import('@/features/knowledge/knowledge-resource-explorer').then((module) => ({ default: module.KnowledgeResourceExplorer })))

const CONVERSATION_RUN_LABELS: Record<ConversationRunIndicator, string> = {
  running: '运行中',
  queued: '运行中',
  approval: '等待审批或回复',
  reply: '等待审批或回复',
  completed: '任务结束',
}

/**
 * The activity slot of one sidebar row. It is always laid out so a conversation
 * starting or finishing a run never shifts the rows around it, and it reports the
 * state as a `status` only when there is something to announce.
 */
function ConversationRunStatus({ indicator, className = 'fox-sidebar-conversation-status' }: { indicator?: ConversationRunIndicator; className?: string }) {
  const label = indicator ? CONVERSATION_RUN_LABELS[indicator] : undefined
  const state = !indicator ? 'idle' : indicator === 'approval' || indicator === 'reply' ? 'waiting' : indicator === 'completed' ? 'complete' : 'running'
  return <span
    className={`${className}${indicator ? ` is-${indicator}` : ''}`}
    role={label ? 'status' : undefined}
    aria-label={label}
    title={label}
    data-run-indicator={indicator ?? undefined}
  >
    <ConversationStatusIcon state={state} size={9.8} />
  </span>
}

export function Sidebar({
  collapsed,
  assistantMode,
  onModeChange,
  onNewChat,
  onNewProjectChat,
  onAddProject,
  onCreateLocalKnowledge,
  onOpenConversation,
  onRenameConversation,
  onPinConversation,
  onPinProject,
  onArchiveConversation,
  onUnarchiveConversation,
  onTrashConversation,
  onRestoreConversation,
  onPurgeConversation,
  onDeleteProject,
  onNavigate,
  onExitManagement,
  onExpand,
  onTheme,
  onSettings,
  runtimeConversations,
  archivedConversations,
  trashedConversations,
  onGlobalSearch,
  activeConversationId,
  newChatActive,
  runIndicators,
  projects = [],
  yuxiService,
  yuxiUser,
  activeView,
  activeEntityId,
  activeDocumentId,
}: {
  collapsed: boolean
  assistantMode: AssistantMode
  onModeChange: (mode: AssistantMode) => void
  onNewChat: () => void
  onNewProjectChat: (projectRoot?: string) => void
  onAddProject: () => void
  onCreateLocalKnowledge?: () => void
  onOpenConversation: (conversationId?: string) => void
  onRenameConversation: (conversation: ConversationSummary) => void
  onPinConversation: (conversation: ConversationSummary) => void
  onPinProject?: (project: SidebarProjectGroup) => void
  onArchiveConversation: (conversation: ConversationSummary) => void
  onUnarchiveConversation: (conversation: ConversationSummary) => void
  onTrashConversation: (conversation: ConversationSummary) => void
  onRestoreConversation: (conversation: ConversationSummary) => void
  onPurgeConversation: (conversation: ConversationSummary) => void
  onDeleteProject: (project: { name: string; root: string; items: Array<{ id: string; title: string }> }) => void
  onNavigate: NavigateWorkspace
  onExitManagement: () => void
  onExpand: () => void
  onTheme: () => void
  onSettings: () => void
  runtimeConversations?: ConversationSummary[]
  archivedConversations?: ConversationSummary[]
  trashedConversations?: ConversationSummary[]
  onGlobalSearch?: (query: string) => Promise<GlobalSearchRecord[]>
  activeConversationId?: string
  newChatActive: boolean
  /**
   * Activity keyed by each conversation's own id. Selection only decides the
   * highlight, never whether a conversation looks busy.
   */
  runIndicators?: ReadonlyMap<string, ConversationRunIndicator>
  projects?: readonly ProjectRecord[]
  yuxiService?: { name: string; status: string; connectionType: 'local' | 'lan' | 'remote' } | null
  yuxiUser?: YuxiUserRecord | null
  activeView: WorkspaceView
  activeEntityId?: string | null
  activeDocumentId?: string | null
}) {
  const [projectOpen, setProjectOpen] = useState<Record<string, boolean>>({})
  const [searchOpen, setSearchOpen] = useState(false)
  const [search, setSearch] = useState('')
  const [searchResults, setSearchResults] = useState<GlobalSearchRecord[] | null>(null)
  const [searchError, setSearchError] = useState<string | null>(null)
  const [lifecycleView, setLifecycleView] = useState<'active' | 'archived' | 'trash'>('active')
  const [pendingSidebarSection, setPendingSidebarSection] = useState<'projects' | 'conversations' | null>(null)
  const [collapsedSection, setCollapsedSection] = useState<'projects' | 'conversations' | null>(null)
  const [collapsedProjectKey, setCollapsedProjectKey] = useState<string | null>(null)
  const [profileMenuOpen, setProfileMenuOpen] = useState(false)
  const [profileDialogOpen, setProfileDialogOpen] = useState(false)
  const [notificationsOpen, setNotificationsOpen] = useState(false)
  const searchInputRef = useRef<HTMLInputElement | null>(null)
  // Element that had focus when the global search opened; restored on close so
  // keyboard users keep their place (Radix's default resolves to <body> here
  // because the dialog is opened imperatively, without a DialogTrigger).
  const searchReturnFocusRef = useRef<HTMLElement | null>(null)
  const projectSectionRef = useRef<HTMLElement | null>(null)
  const conversationSectionRef = useRef<HTMLElement | null>(null)
  const normalizedSearch = search.trim().toLocaleLowerCase()
  const changeSearchOpen = useCallback((open: boolean) => {
    setSearchOpen(open)
    if (open) {
      const active = document.activeElement
      searchReturnFocusRef.current = active instanceof HTMLElement && active !== document.body ? active : null
      window.requestAnimationFrame(() => searchInputRef.current?.focus())
      return
    }
    setSearch('')
    setSearchResults(null)
    setSearchError(null)
  }, [])
  useEffect(() => {
    const openSearchFromShortcut = (event: globalThis.KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey || event.key.toLocaleLowerCase() !== 'k') return
      event.preventDefault()
      changeSearchOpen(true)
    }
    window.addEventListener('keydown', openSearchFromShortcut)
    return () => window.removeEventListener('keydown', openSearchFromShortcut)
  }, [changeSearchOpen])
  useEffect(() => {
    if (!normalizedSearch) {
      setSearchResults(null)
      setSearchError(null)
      return
    }
    if (!onGlobalSearch) {
      setSearchResults([])
      setSearchError('当前环境暂不支持全局搜索')
      return
    }
    setSearchResults(null)
    setSearchError(null)
    let cancelled = false
    const timer = window.setTimeout(() => {
      void onGlobalSearch(search.trim()).then((records) => {
        if (!cancelled) setSearchResults(records)
      }).catch((cause) => {
        if (!cancelled) setSearchError(cause instanceof Error ? cause.message : '全局搜索失败，请稍后重试')
      })
    }, 180)
    return () => {
      cancelled = true
      window.clearTimeout(timer)
    }
  }, [normalizedSearch, onGlobalSearch, search])
  const lifecycleConversations = lifecycleView === 'archived'
    ? archivedConversations
    : lifecycleView === 'trash'
      ? trashedConversations
      : runtimeConversations
  const conversationItems = lifecycleConversations?.map((item) => ({
    ...item,
    time: item.lastMessageAt ? new Date(item.lastMessageAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '',
    active: item.id === activeConversationId
  })) ?? []
  const projectConversationItems = lifecycleView === 'active'
    ? conversationItems.filter((item) => Boolean(item.projectRoot))
    : []
  const regularConversationItems = lifecycleView === 'active'
    ? conversationItems.filter((item) => !item.projectRoot)
    : conversationItems
  const visibleProjectConversations = projectConversationItems
  const projectGroups = sidebarProjectGroups(visibleProjectConversations, projects)
  const visibleConversations = regularConversationItems
  const toggleSearch = () => changeSearchOpen(!searchOpen)
  const changeNotificationsOpen = (open: boolean) => {
    setNotificationsOpen(open)
    if (open) changeSearchOpen(false)
  }
  const groupedSearchResults = Array.from((searchResults ?? []).reduce((groups, item) => {
    const group = groups.get(item.kind) ?? []
    group.push(item)
    groups.set(item.kind, group)
    return groups
  }, new Map<GlobalSearchRecord['kind'], GlobalSearchRecord[]>()).entries())
  const openGlobalSearchResult = (item: GlobalSearchRecord) => {
    changeSearchOpen(false)
    if (['conversation', 'artifact', 'task'].includes(item.kind) && item.entityId) {
      onOpenConversation(item.entityId)
      return
    }
    onNavigate(item.workspaceView as WorkspaceView, item.entityId ?? undefined, item.documentId ? { documentId: item.documentId } : undefined)
  }
  const setAllProjectsOpen = (open: boolean) => {
    setProjectOpen(Object.fromEntries(projectGroups.map((project) => [project.key, open])))
  }
  const revealSidebarSection = (section: 'projects' | 'conversations') => {
    setLifecycleView('active')
    setPendingSidebarSection(section)
    onExpand()
  }
  useEffect(() => {
    if (collapsed || !pendingSidebarSection) return
    const frame = window.requestAnimationFrame(() => {
      const target = pendingSidebarSection === 'projects' ? projectSectionRef.current : conversationSectionRef.current
      target?.scrollIntoView({ block: 'start' })
      setPendingSidebarSection(null)
    })
    return () => window.cancelAnimationFrame(frame)
  }, [collapsed, pendingSidebarSection])
  const { profile, save: saveProfile } = useUserProfile({ name: yuxiUser?.username ?? 'Hury', avatar: yuxiUser?.avatar })
  const profileName = profile.name
  const profileInitial = profile.initial
  const profileDetail = yuxiUser
    ? [yuxiUser.departmentName, yuxiUser.role].filter(Boolean).join(' · ')
    : 'Fox 本地用户'
  const agentViews: WorkspaceView[] = ['agents', 'agent-detail']
  const knowledgeViews: WorkspaceView[] = ['knowledge', 'knowledge-detail', 'knowledge-graph']
  const knowledgeDetailViews: WorkspaceView[] = ['knowledge-detail', 'knowledge-graph']
  const sidebarMode = SETTINGS_WORKSPACE_VIEWS.includes(activeView)
    ? 'settings'
    : LOCAL_KNOWLEDGE_WORKSPACE_VIEWS.includes(activeView)
      ? 'local-knowledge'
    : activeView === 'agent-detail'
      ? 'agent'
    : knowledgeDetailViews.includes(activeView)
      ? 'knowledge'
      : 'chat'
  const showSidebarUtilities = sidebarMode === 'chat' || sidebarMode === 'local-knowledge'
  useEffect(() => {
    if (!showSidebarUtilities) setNotificationsOpen(false)
  }, [showSidebarUtilities])
  const agentSection = activeDocumentId ?? 'home'
  const agentContextItems = [
    { section: 'home', label: '专家详情', icon: House },
    { section: 'knowledge', label: '知识库', icon: Database },
    { section: 'skills', label: '技能', icon: WandSparkles },
    { section: 'mcp', label: '连接器', icon: Cable },
    { section: 'tools', label: '工具', icon: Wrench },
    { section: 'conversations', label: '对话任务', icon: ClipboardList },
    { section: 'memory', label: '记忆', icon: Brain },
  ]
  const activeSettingsView: WorkspaceView = activeView === 'onboarding'
    ? 'settings'
    : activeView === 'model-service'
    ? 'settings-models'
    : activeView === 'service' || activeView === 'login'
      ? 'settings-yuxi'
      : activeView === 'skills' || activeView === 'mcp'
        ? 'settings-extensions'
        : activeView
  const settingsContextGroups = [
    {
      label: '通用',
      items: [
        { view: 'settings' as const, label: '应用', icon: Settings },
        { view: 'settings-conversation' as const, label: '输入与对话', icon: MessageCircleMore },
      ],
    },
    {
      label: '模型与知识',
      items: [
        { view: 'settings-models' as const, label: '模型供应商', icon: Server },
        { view: 'settings-ai' as const, label: '助手设置', icon: Bot },
        { view: 'settings-yuxi' as const, label: '知识库服务', icon: Globe2 },
      ],
    },
    {
      label: '权限与数据',
      items: [
        { view: 'settings-projects' as const, label: '项目与权限', icon: ShieldCheck },
        { view: 'settings-extensions' as const, label: '扩展', icon: Wrench },
        { view: 'settings-usage' as const, label: '使用统计', icon: Activity },
        { view: 'maintenance' as const, label: '数据与诊断', icon: ShieldCheck },
      ],
    },
    {
      label: '关于',
      items: [
        { view: 'settings-about' as const, label: '关于 Fox', icon: CircleHelp },
      ],
    },
  ]
  const contextItems = [
    { view: 'knowledge-detail' as const, label: '文件与详情', icon: BookOpen },
    { view: 'knowledge-graph' as const, label: '知识图谱', icon: Globe2 },
  ]

  return (
    <aside className={`fox-sidebar ${collapsed ? 'is-collapsed' : ''} ${!['chat', 'local-knowledge'].includes(sidebarMode) ? 'is-management' : ''} ${notificationsOpen ? 'is-notifications' : ''}`}>
      <div className="fox-sidebar-title-safe">
        {collapsed && (sidebarMode === 'chat' || sidebarMode === 'local-knowledge') ? <Tooltip>
          <TooltipTrigger asChild><button type="button" className="fox-sidebar-mode-switch" aria-label={assistantMode === 'assistant' ? '切换到知识' : '切换到助手'} onClick={() => onModeChange(assistantMode === 'assistant' ? 'knowledge' : 'assistant')}>{assistantMode === 'assistant' ? <BookCopy /> : <Sparkles />}</button></TooltipTrigger>
          <TooltipContent side="right"><p>{assistantMode === 'assistant' ? '切换到知识' : '切换到助手'}</p></TooltipContent>
        </Tooltip> : !collapsed && !['chat', 'local-knowledge'].includes(sidebarMode) ? <button type="button" className="fox-management-back" onClick={sidebarMode === 'agent' ? () => onNavigate('agents') : onExitManagement}><ArrowLeft size={16} /><span>返回</span></button> : <span className="fox-window-safe" />}
      </div>

      {showSidebarUtilities && !collapsed && (
        <div className="fox-sidebar-mode-row">
          <Tabs value={assistantMode} onValueChange={(value) => onModeChange(value as AssistantMode)} className="fox-mode-tabs">
            <TabsList>
              <TabsTrigger value="assistant"><Sparkles />助手</TabsTrigger>
              <TabsTrigger value="knowledge"><BookCopy />知识</TabsTrigger>
            </TabsList>
          </Tabs>
          <div className="fox-sidebar-mode-actions"><button type="button" className={`fox-sidebar-search-trigger ${searchOpen ? 'is-active' : ''}`} onClick={toggleSearch} aria-label="全局搜索" title="搜索（Ctrl K）"><Search /></button></div>
        </div>
      )}

      {sidebarMode === 'chat' ? <><div className="fox-sidebar-primary">
        <button className={`fox-sidebar-command is-emphasis ${newChatActive ? 'is-active' : ''}`} onClick={onNewChat}><MessageSquarePlus size={18} /><span>新建对话</span><kbd>Ctrl N</kbd></button>
        <button className={`fox-sidebar-command starts-destination-group ${agentViews.includes(activeView) ? 'is-active' : ''}`} onClick={() => onNavigate('agents')}><UserRoundCheck size={18} /><span>专家</span></button>
        <button className={`fox-sidebar-command ${knowledgeViews.includes(activeView) ? 'is-active' : ''}`} onClick={() => onNavigate('knowledge')}><Library size={18} /><span>远程知识库</span></button>
        <button className={`fox-sidebar-command ${activeView === 'plugins' ? 'is-active' : ''}`} onClick={() => onNavigate('plugins')}><PackagePlus size={18} /><span>插件</span></button>
      </div>

      {!notificationsOpen && !collapsed && (
        <ScrollArea className="fox-sidebar-scroll">
          {lifecycleView === 'active' && <section ref={projectSectionRef} className="fox-sidebar-section">
            <div className="fox-section-head"><span>项目</span><span><IconButton label="添加项目" onClick={onAddProject}><Plus size={14} /></IconButton><DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多项目操作"><MoreHorizontal size={14} /></Button></DropdownMenuTrigger><DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-project-menu"><DropdownMenuItem onSelect={onAddProject}><Plus />添加项目</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem disabled={!projectGroups.length} onSelect={() => setAllProjectsOpen(true)}><ChevronDown />全部展开</DropdownMenuItem><DropdownMenuItem disabled={!projectGroups.length} onSelect={() => setAllProjectsOpen(false)}><ChevronRight />全部收起</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem onSelect={() => onNavigate('settings-projects')}><ShieldCheck />项目与权限</DropdownMenuItem></DropdownMenuContent></DropdownMenu></span></div>
            {projectGroups.length ? projectGroups.map((project) => {
              const open = projectOpen[project.key] ?? true
              return <div className="fox-project-group" key={project.key}>
                <ContextMenu>
                  <ContextMenuTrigger asChild>
                    {/* The project row is a container, not a selected row: a child
                        conversation being active must not light up its project
                        as well, or the sidebar shows two selections at once. */}
                    <div className="fox-project-row">
                      <button type="button" className="fox-project-row-main" title={project.root} aria-expanded={open} onClick={() => setProjectOpen((current) => ({ ...current, [project.key]: !open }))}>
                        {open ? <FolderOpen size={15} /> : <Folder size={15} />}
                        <strong>{project.name}</strong>
                        {project.pinned && <Pin size={11} aria-label="已置顶项目" />}
                      </button>
                      <span className="fox-project-row-actions">
                        <IconButton label={`在 ${project.name} 中新建对话`} className="fox-project-add" onClick={() => onNewProjectChat(project.root)}><Plus size={13} /></IconButton>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label={`更多 ${project.name} 项目操作`}><MoreHorizontal size={13} /></Button></DropdownMenuTrigger>
                          <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-project-menu">
                            <DropdownMenuItem onSelect={() => onNewProjectChat(project.root)}><MessageSquarePlus />新建对话</DropdownMenuItem>
                            <DropdownMenuItem onSelect={() => onNavigate('settings-projects')}><ShieldCheck />项目与权限</DropdownMenuItem>
                            <DropdownMenuItem disabled={!project.id} onSelect={() => onPinProject?.(project)}>{project.pinned ? <PinOff /> : <Pin />}{project.pinned ? '取消置顶项目' : '置顶项目'}</DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem variant="destructive" onSelect={() => onDeleteProject(project)}><Trash2 />删除项目</DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </span>
                    </div>
                  </ContextMenuTrigger>
                  <ContextMenuContent className="fox-project-menu">
                    <ContextMenuItem onSelect={() => onNewProjectChat(project.root)}><MessageSquarePlus />新建对话</ContextMenuItem>
                    <ContextMenuItem onSelect={() => onNavigate('settings-projects')}><ShieldCheck />项目与权限</ContextMenuItem>
                    <ContextMenuItem disabled={!project.id} onSelect={() => onPinProject?.(project)}>{project.pinned ? <PinOff /> : <Pin />}{project.pinned ? '取消置顶项目' : '置顶项目'}</ContextMenuItem>
                    <ContextMenuSeparator />
                    <ContextMenuItem variant="destructive" onSelect={() => onDeleteProject(project)}><Trash2 />删除项目</ContextMenuItem>
                  </ContextMenuContent>
                </ContextMenu>
                {open && <div className="fox-project-children">{project.items.map((item) => (
                  <ContextMenu key={item.id}>
                    <ContextMenuTrigger asChild>
                      <div className={`fox-sidebar-tree-row fox-project-conversation-row ${item.active ? 'is-active' : ''}`}>
                        <button type="button" className="fox-sidebar-tree-main" title={item.title} onClick={() => onOpenConversation(item.id)}><span>{item.title}</span></button>
                        <ConversationRunStatus indicator={runIndicators?.get(item.id)} />
                        <span className="fox-sidebar-row-time">{item.time}</span>
                        <span className="fox-sidebar-row-actions">
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多项目对话操作"><MoreHorizontal size={13} /></Button></DropdownMenuTrigger>
                            <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-conversation-menu">
                              <DropdownMenuItem onSelect={() => onPinConversation(item)}>{item.pinned ? <PinOff /> : <Pin />}{item.pinned ? '取消置顶' : '置顶对话'}</DropdownMenuItem>
                              <DropdownMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名对话</DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem onSelect={() => onArchiveConversation(item)}><Archive />归档对话</DropdownMenuItem>
                              <DropdownMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />移入回收站</DropdownMenuItem>
                            </DropdownMenuContent>
                          </DropdownMenu>
                        </span>
                      </div>
                    </ContextMenuTrigger>
                    <ContextMenuContent className="fox-conversation-menu">
                      <ContextMenuItem onSelect={() => onPinConversation(item)}>{item.pinned ? <PinOff /> : <Pin />}{item.pinned ? '取消置顶' : '置顶对话'}</ContextMenuItem>
                      <ContextMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名对话</ContextMenuItem>
                      <ContextMenuSeparator />
                      <ContextMenuItem onSelect={() => onArchiveConversation(item)}><Archive />归档对话</ContextMenuItem>
                      <ContextMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />移入回收站</ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                ))}</div>}
              </div>
            }) : <p className="fox-project-empty">暂无项目对话</p>}
          </section>}

          <section ref={conversationSectionRef} className="fox-sidebar-section fox-conversation-section">
            <div className="fox-section-head">
              <span>{lifecycleView === 'archived' ? '已归档' : lifecycleView === 'trash' ? '回收站' : '对话'}</span>
              <span>
                {lifecycleView === 'active' && <IconButton label="新建会话" onClick={onNewChat}><Plus size={14} /></IconButton>}
                <DropdownMenu>
                  <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="切换对话列表"><MoreHorizontal size={14} /></Button></DropdownMenuTrigger>
                  <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-conversation-menu">
                    <DropdownMenuItem onSelect={() => setLifecycleView('active')}><MessageCircleMore />当前对话</DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => setLifecycleView('archived')}><Archive />已归档</DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => setLifecycleView('trash')}><Trash2 />回收站</DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </span>
            </div>
            <div className="fox-conversation-list">
              {visibleConversations.map((item) => (
                <ContextMenu key={item.id}>
                  <ContextMenuTrigger asChild>
                    <div className={`fox-sidebar-tree-row ${item.active ? 'is-active' : ''}`}>
                      <button type="button" className="fox-sidebar-tree-main" title={item.title} onClick={() => onOpenConversation(item.id)}><MessageCircleMore size={14} /><span>{item.title}</span></button>
                      <ConversationRunStatus indicator={runIndicators?.get(item.id)} />
                      <span className="fox-sidebar-row-time">{item.time}</span>
                      <span className="fox-sidebar-row-actions">
                        <DropdownMenu>
                          <Tooltip><TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多会话操作"><MoreHorizontal size={13} /></Button></DropdownMenuTrigger></TooltipTrigger><TooltipContent side="right"><p>更多会话操作</p></TooltipContent></Tooltip>
                          <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-conversation-menu">
                            {lifecycleView === 'active' ? <>
                              <DropdownMenuItem onSelect={() => onPinConversation(item)}>{item.pinned ? <PinOff /> : <Pin />}{item.pinned ? '取消置顶' : '置顶对话'}</DropdownMenuItem>
                              <DropdownMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名对话</DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem onSelect={() => onArchiveConversation(item)}><Archive />归档对话</DropdownMenuItem>
                              <DropdownMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />移入回收站</DropdownMenuItem>
                            </> : lifecycleView === 'archived' ? <>
                              <DropdownMenuItem onSelect={() => onUnarchiveConversation(item)}><RotateCcw />恢复到对话</DropdownMenuItem>
                              <DropdownMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />移入回收站</DropdownMenuItem>
                            </> : <>
                              <DropdownMenuItem onSelect={() => onRestoreConversation(item)}><RotateCcw />恢复对话</DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem variant="destructive" onSelect={() => onPurgeConversation(item)}><Trash2 />永久删除</DropdownMenuItem>
                            </>}
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </span>
                    </div>
                  </ContextMenuTrigger>
                  <ContextMenuContent className="fox-conversation-menu">
                  {lifecycleView === 'active' ? <>
                    <ContextMenuItem onSelect={() => onPinConversation(item)}>{item.pinned ? <PinOff /> : <Pin />}{item.pinned ? '取消置顶' : '置顶对话'}</ContextMenuItem>
                    <ContextMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名对话</ContextMenuItem>
                    <ContextMenuSeparator />
                    <ContextMenuItem onSelect={() => onArchiveConversation(item)}><Archive />归档对话</ContextMenuItem>
                    <ContextMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />移入回收站</ContextMenuItem>
                  </> : lifecycleView === 'archived' ? <>
                    <ContextMenuItem onSelect={() => onUnarchiveConversation(item)}><RotateCcw />恢复到对话</ContextMenuItem>
                    <ContextMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />移入回收站</ContextMenuItem>
                  </> : <>
                    <ContextMenuItem onSelect={() => onRestoreConversation(item)}><RotateCcw />恢复对话</ContextMenuItem>
                    <ContextMenuSeparator />
                    <ContextMenuItem variant="destructive" onSelect={() => onPurgeConversation(item)}><Trash2 />永久删除</ContextMenuItem>
                  </>}
                  </ContextMenuContent>
                </ContextMenu>
              ))}
              {!visibleConversations.length && <p className="fox-project-empty">{lifecycleView === 'archived' ? '暂无归档对话' : lifecycleView === 'trash' ? '回收站为空' : '暂无对话'}</p>}
            </div>
          </section>
        </ScrollArea>
      )}</> : sidebarMode === 'local-knowledge' ? <LocalKnowledgeSidebarNavigation collapsed={collapsed} activeView={activeView} activeEntityId={activeEntityId} activeDocumentId={activeDocumentId} navigate={onNavigate} hideLibrary={notificationsOpen} onCreate={() => { if (onCreateLocalKnowledge) onCreateLocalKnowledge(); else onNavigate('local-knowledge') }} /> : !notificationsOpen ? <>
        <div className={`fox-sidebar-primary fox-context-sidebar-nav ${sidebarMode === 'settings' ? 'is-settings' : ''}`}>
          {sidebarMode === 'agent'
            ? agentContextItems.map(({ section, label, icon: Icon }) => <button key={section} className={`fox-sidebar-command ${agentSection === section ? 'is-active' : ''}`} onClick={() => onNavigate('agent-detail', activeEntityId ?? undefined, { documentId: section })}><Icon size={16} /><span>{label}</span></button>)
            : sidebarMode === 'settings'
              ? settingsContextGroups.map((group) => <section className="fox-context-sidebar-group" key={group.label}>{!collapsed && <span className="fox-context-sidebar-group-label">{group.label}</span>}{group.items.map(({ view, label, icon: Icon }) => <button key={view} className={`fox-sidebar-command ${activeSettingsView === view ? 'is-active' : ''}`} onClick={() => onNavigate(view)}><Icon size={18} /><span>{label}</span></button>)}</section>)
              : contextItems.map(({ view, label, icon: Icon }) => <button key={view} className={`fox-sidebar-command ${activeView === view ? 'is-active' : ''}`} onClick={() => onNavigate(view, activeEntityId ?? undefined)}><Icon size={18} /><span>{label}</span></button>)}
        </div>
        {!collapsed && sidebarMode === 'knowledge' && (
          <Suspense fallback={null}><KnowledgeResourceExplorer
            knowledgeId={activeEntityId}
            selectedDocumentId={activeDocumentId}
            navigate={onNavigate}
          /></Suspense>
        )}
      </> : null}

      <NotificationCenter
        onNavigate={onNavigate}
        open={notificationsOpen}
        onOpenChange={changeNotificationsOpen}
        showTrigger={showSidebarUtilities && !collapsed}
        activeView={activeView}
        activeEntityId={activeEntityId}
        activeConversationId={activeConversationId}
      />

      <div className="fox-sidebar-spacer" />
      {!notificationsOpen && collapsed && sidebarMode === 'chat' && <div className="fox-sidebar-collapsed-sections" aria-label="项目与对话">
        <div
          className="fox-sidebar-collapsed-entry"
          onMouseEnter={() => setCollapsedSection('projects')}
          onMouseLeave={() => { setCollapsedSection(null); setCollapsedProjectKey(null) }}
        >
          <button
            type="button"
            className={collapsedSection === 'projects' ? 'is-active' : undefined}
            aria-label={`展开项目列表，共 ${projectGroups.length} 个项目`}
            title={`项目 · ${projectGroups.length}`}
            onFocus={() => setCollapsedSection('projects')}
            onClick={() => revealSidebarSection('projects')}
          >
            <Folders />
            <span className="fox-sidebar-collapsed-count">{projectGroups.length}</span>
          </button>
          {collapsedSection === 'projects' && <div className="fox-sidebar-collapsed-popover fox-sidebar-project-popover" role="menu" aria-label="项目列表">
            <header>项目</header>
            {projectGroups.length ? projectGroups.map((project) => <div
              className="fox-sidebar-hover-project"
              key={project.key}
              onMouseEnter={() => setCollapsedProjectKey(project.key)}
              onFocus={() => setCollapsedProjectKey(project.key)}
            >
              <button
                type="button"
                role="menuitem"
                title={project.root}
                onClick={() => { const firstConversation = project.items[0]; if (firstConversation) onOpenConversation(firstConversation.id) }}
              >
                <FolderOpen />
                <span>{project.name}</span>
                <small>{project.items.length}</small>
                <ChevronRight className="fox-sidebar-hover-project-chevron" />
              </button>
              {collapsedProjectKey === project.key && <div className="fox-sidebar-collapsed-popover fox-sidebar-conversations-popover" role="menu" aria-label={`${project.name} 对话`}>
                <header>{project.name}</header>
                {project.items.map((item) => {
                  const indicator = runIndicators?.get(item.id)
                  return <button type="button" role="menuitem" key={item.id} className={item.active ? 'is-active' : undefined} onClick={() => onOpenConversation(item.id)} title={indicator ? `${item.title} · ${CONVERSATION_RUN_LABELS[indicator]}` : item.title}>
                  <MessageCircleMore />
                  <span>{item.title}</span>
                  <ConversationRunStatus indicator={indicator} className="fox-sidebar-collapsed-run" />
                  {item.time && <time>{item.time}</time>}
                </button>
                })}
              </div>}
            </div>) : <p className="fox-sidebar-collapsed-empty">暂无项目</p>}
          </div>}
        </div>
        <div
          className="fox-sidebar-collapsed-entry"
          onMouseEnter={() => setCollapsedSection('conversations')}
          onMouseLeave={() => setCollapsedSection(null)}
        >
          <button
            type="button"
            className={collapsedSection === 'conversations' ? 'is-active' : undefined}
            aria-label={`展开对话列表，共 ${visibleConversations.length} 个对话`}
            title={`对话 · ${visibleConversations.length}`}
            onFocus={() => setCollapsedSection('conversations')}
            onClick={() => revealSidebarSection('conversations')}
          >
            <MessageCircleMore />
            <span className="fox-sidebar-collapsed-count">{visibleConversations.length}</span>
          </button>
          {collapsedSection === 'conversations' && <div className="fox-sidebar-collapsed-popover fox-sidebar-conversations-list-popover" role="menu" aria-label="对话列表">
            <header>对话</header>
            {visibleConversations.length ? visibleConversations.map((item) => {
              const indicator = runIndicators?.get(item.id)
              return <button type="button" role="menuitem" key={item.id} className={item.active ? 'is-active' : undefined} onClick={() => onOpenConversation(item.id)} title={indicator ? `${item.title} · ${CONVERSATION_RUN_LABELS[indicator]}` : item.title}>
              <MessageCircleMore />
              <span>{item.title}</span>
              <ConversationRunStatus indicator={indicator} className="fox-sidebar-collapsed-run" />
              {item.time && <time>{item.time}</time>}
            </button>
            }) : <p className="fox-sidebar-collapsed-empty">暂无对话</p>}
          </div>}
        </div>
      </div>}
      <div className="fox-sidebar-footer">
        <DropdownMenu open={profileMenuOpen} onOpenChange={setProfileMenuOpen}>
          <Tooltip>
            <TooltipTrigger asChild>
              <DropdownMenuTrigger asChild>
                <button type="button" className="fox-profile" aria-label="打开用户菜单">
                  <Avatar><AvatarImage src={profile.avatar} alt="" /><AvatarFallback>{profileInitial}</AvatarFallback></Avatar>
                  <span className="fox-profile-copy"><strong>{profileName}</strong></span>
                </button>
              </DropdownMenuTrigger>
            </TooltipTrigger>
            <TooltipContent side={collapsed ? 'right' : 'top'}><p>用户菜单</p></TooltipContent>
          </Tooltip>
          <DropdownMenuContent
            side={collapsed ? 'right' : 'top'}
            align="start"
            sideOffset={collapsed ? 8 : 10}
            className="fox-profile-menu"
          >
            <DropdownMenuLabel className="fox-profile-menu-head">
              <Avatar><AvatarImage src={profile.avatar} alt="" /><AvatarFallback>{profileInitial}</AvatarFallback></Avatar>
              <span className="fox-profile-menu-copy"><strong>{profileName}</strong><small>{profileDetail}</small></span>
              <button type="button" className="fox-profile-edit" aria-label="编辑个人资料" onPointerDown={(event) => event.stopPropagation()} onClick={(event) => { event.stopPropagation(); setProfileMenuOpen(false); setProfileDialogOpen(true) }}><FileEdit /></button>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => setProfileDialogOpen(true)}><UserRound />个人资料</DropdownMenuItem>
            <DropdownMenuItem onSelect={onSettings}><Settings />账户与偏好</DropdownMenuItem>
            <DropdownMenuSeparator />
            <div className="fox-profile-menu-status"><i className={yuxiService?.status === 'connected' && yuxiUser ? '' : 'is-muted'} /><span className="fox-profile-menu-copy"><strong>远程知识库</strong></span></div>
          </DropdownMenuContent>
        </DropdownMenu>
        <IconButton label="切换主题" onClick={onTheme}><SunMoon size={16} /></IconButton>
        <IconButton label="设置" onClick={onSettings}><Settings size={16} /></IconButton>
      </div>
      <Dialog open={searchOpen} onOpenChange={changeSearchOpen}>
        <DialogContent
          className="fox-global-search-dialog"
          showCloseButton={false}
          onCloseAutoFocus={(event) => {
            const target = searchReturnFocusRef.current
            searchReturnFocusRef.current = null
            if (!target || !target.isConnected) return
            event.preventDefault()
            target.focus()
          }}
        >
          <DialogHeader className="sr-only"><DialogTitle>全局搜索</DialogTitle><DialogDescription>搜索对话、项目、知识库、文件和任务。</DialogDescription></DialogHeader>
          <label className="fox-global-search-input-shell">
            <Search aria-hidden="true" />
            <input ref={searchInputRef} autoFocus value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索对话、项目、知识库、文件和任务" />
            {search && <button type="button" onClick={() => setSearch('')} aria-label="清除搜索"><X /></button>}
          </label>
          {normalizedSearch && <div className="fox-global-search-results" role="region" aria-label="全局搜索结果" aria-live="polite">
            {!searchError && searchResults === null && <div className="fox-global-search-state"><LoaderCircle className="animate-spin" />正在搜索</div>}
            {searchError && <div className="fox-global-search-state is-error"><AlertTriangle />{searchError}</div>}
            {!searchError && searchResults?.length === 0 && <div className="fox-global-search-state"><Search />没有匹配结果</div>}
            {!searchError && groupedSearchResults.map(([resultKind, records]) => <section key={resultKind}><header>{globalSearchKindLabel(resultKind)}<span>{records.length}</span></header>{records.map((item) => <button type="button" key={`${item.kind}:${item.id}`} onClick={() => openGlobalSearchResult(item)}><strong>{item.title}</strong><small>{item.subtitle}</small><time>{new Date(item.updatedAt).toLocaleDateString('zh-CN')}</time></button>)}</section>)}
          </div>}
        </DialogContent>
      </Dialog>
      <UserProfileDialog open={profileDialogOpen} onOpenChange={setProfileDialogOpen} profile={profile} detail={profileDetail} onSave={async (next) => { await saveProfile(next); toast.success('个人资料已更新') }} />
    </aside>
  )
}

export const MemoizedSidebar = memo(Sidebar)
