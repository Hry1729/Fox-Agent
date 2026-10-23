import { sidebarDateGroups } from '../sidebar-date-groups'
import '@/components/fox-ui/sidebar-list-pane.css'
import { type ConversationRunIndicator } from '../conversation-run-indicator'
import { ConversationAttentionBadge, ConversationRunStatus } from '../conversation-run-status'
import { SidebarCollapsedMenus } from './fox-sidebar-collapsed-menus'
import { type SidebarProjectGroup, sidebarProjectGroups } from '../sidebar-project-groups'
import { Button } from '@/components/ui/button'
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from '@/components/ui/context-menu'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { ScrollArea } from '@/components/ui/scroll-area'
import { SidebarModeTabs, SidebarModeTab, UserMenu } from './fox-sidebar-controls'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { AnimatedIconTrigger, Bell, Blocks, BookCopy, MessageCirclePlus, Search, Settings, Sparkles, SunMoon, Users } from '@/components/fox-ui/animated-icons'
import { SidebarMotion, SidebarMotionItem } from './fox-sidebar-motion'
import './fox-sidebar.css'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { type ConversationSummary, type GlobalSearchRecord, type ProjectRecord, type YuxiUserRecord } from '@/features/conversations/model/types'
import { LocalKnowledgeSidebarNavigation } from '@/features/local-knowledge/local-knowledge-sidebar'
import { notify as toast } from '@/features/notifications'
import { UserProfileDialog, useUserProfile } from '@/features/profile/user-profile'
import { remoteKnowledgeBackRoute, SETTINGS_WORKSPACE_VIEWS } from '@/features/workspace/management-navigation'
import { detectShortcutPlatform, formatShortcut, matchesShortcutAction, shortcutTokens, type ShortcutActionId } from '@/features/settings/shortcut-model'
import { useShortcutPreferences } from '@/features/settings/use-shortcuts'
import { type NavigateWorkspace, type WorkspaceView } from '@/features/workspace/types'
import { Activity, AlertTriangle, Archive, ArrowLeft, BookOpen, Bot, Brain, Cable, ChevronDown, ChevronRight, CircleHelp, ClipboardList, Database, FileEdit, Folder, FolderOpen, Globe2, House, Keyboard, LoaderCircle, MessageCircleMore, MoreHorizontal, Pin, PinOff, Plus, RotateCcw, Server, ShieldCheck, Trash2, UserRound, WandSparkles, Wrench, X } from 'lucide-react'
import { Suspense, lazy, memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { type AssistantMode, IconButton, globalSearchKindLabel } from '../fox-shared'
import { NotificationCenter } from './fox-notification-center'
import { LOCAL_KNOWLEDGE_WORKSPACE_VIEWS } from '../workbench'

const EMPTY_PROJECTS: readonly ProjectRecord[] = []

const KnowledgeResourceExplorer = lazy(() => import('@/features/knowledge/knowledge-resource-explorer').then((module) => ({ default: module.KnowledgeResourceExplorer })))

export function Sidebar({
  collapsed,
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
  onTheme,
  onSettings,
  runtimeConversations,
  archivedConversations,
  trashedConversations,
  onGlobalSearch,
  activeConversationId,
  newChatActive,
  runIndicators,
  projects = EMPTY_PROJECTS,
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
  const { preferences: shortcutPreferences } = useShortcutPreferences()
  const shortcutPlatform = detectShortcutPlatform()
  const shortcutLabel = (actionId: ShortcutActionId) => {
    const binding = shortcutPreferences[actionId]
    return binding ? formatShortcut(binding, shortcutPlatform).text : null
  }
  const newConversationShortcut = shortcutLabel('new-conversation')
  const searchShortcut = shortcutLabel('search')
  const searchTitle = searchShortcut ? `搜索（${searchShortcut}）` : '搜索'
  const settingsBinding = shortcutPreferences['open-settings']
  const settingsKeys = settingsBinding ? shortcutTokens(settingsBinding, shortcutPlatform).map((token) => token.label) : undefined
  const [projectOpen, setProjectOpen] = useState<Record<string, boolean>>({})
  const [searchOpen, setSearchOpen] = useState(false)
  const [search, setSearch] = useState('')
  const [searchResults, setSearchResults] = useState<GlobalSearchRecord[] | null>(null)
  const [searchAttempt, setSearchAttempt] = useState(0)
  const [searchError, setSearchError] = useState<string | null>(null)
  const [lifecycleView, setLifecycleView] = useState<'active' | 'archived' | 'trash'>('active')
  const [profileMenuOpen, setProfileMenuOpen] = useState(false)
  const [profileDialogOpen, setProfileDialogOpen] = useState(false)
  const [notificationsOpen, setNotificationsOpen] = useState(false)
  const searchInputRef = useRef<HTMLInputElement | null>(null)
  // Element that had focus when the global search opened; restored on close so
  // keyboard users keep their place (Radix's default resolves to <body> here
  // because the dialog is opened imperatively, without a DialogTrigger).
  const searchReturnFocusRef = useRef<HTMLElement | null>(null)
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
      if (!matchesShortcutAction(event, 'search')) return
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
  }, [normalizedSearch, onGlobalSearch, search, searchAttempt])
  const lifecycleConversations = lifecycleView === 'archived'
    ? archivedConversations
    : lifecycleView === 'trash'
      ? trashedConversations
      : runtimeConversations
  const conversationItems = useMemo(() => {
    const timeFormat = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' })
    return lifecycleConversations?.map((item) => ({
      ...item,
      time: item.lastMessageAt ? timeFormat.format(new Date(item.lastMessageAt)) : '',
      active: item.id === activeConversationId
    })) ?? []
  }, [lifecycleConversations, activeConversationId])
  const projectGroups = useMemo(() => sidebarProjectGroups(
    lifecycleView === 'active' ? conversationItems.filter(item => Boolean(item.projectRoot)) : [], projects
  ), [conversationItems, lifecycleView, projects])
  const visibleConversations = useMemo(() => lifecycleView === 'active'
    ? conversationItems.filter(item => !item.projectRoot)
    : conversationItems, [conversationItems, lifecycleView])
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
  const setAllProjectsOpen = useCallback((open: boolean) => {
    setProjectOpen(Object.fromEntries(projectGroups.map((project) => [project.key, open])))
  }, [projectGroups])
  const { profile, save: saveProfile } = useUserProfile({ name: yuxiUser?.username ?? 'Hury', avatar: yuxiUser?.avatar })
  const profileName = profile.name
  const profileDetail = yuxiUser
    ? [yuxiUser.departmentName, yuxiUser.role].filter(Boolean).join(' · ')
    : 'Fox 本机用户'
  const agentViews: WorkspaceView[] = ['agents', 'agent-detail']
  const knowledgeViews: WorkspaceView[] = ['knowledge', 'knowledge-detail', 'knowledge-graph']
  const knowledgeDetailViews: WorkspaceView[] = ['knowledge-detail', 'knowledge-graph']
  const selectedMode: AssistantMode = LOCAL_KNOWLEDGE_WORKSPACE_VIEWS.includes(activeView) || knowledgeViews.includes(activeView) ? 'knowledge' : 'assistant'
  const sidebarMode = SETTINGS_WORKSPACE_VIEWS.includes(activeView)
    ? 'settings'
    : LOCAL_KNOWLEDGE_WORKSPACE_VIEWS.includes(activeView) || knowledgeViews.includes(activeView)
      ? 'local-knowledge'
    : activeView === 'agent-detail'
      ? 'agent'
    : knowledgeDetailViews.includes(activeView)
      ? 'knowledge'
      : 'chat'
  const showSidebarUtilities = sidebarMode === 'chat' || sidebarMode === 'local-knowledge'
  const goBack = () => {
    if (sidebarMode === 'agent') onNavigate('agents')
    else if (sidebarMode === 'knowledge') {
      const route = remoteKnowledgeBackRoute(activeView, activeEntityId)
      onNavigate(route.view, route.entityId)
    } else onExitManagement()
  }
  useEffect(() => {
    if (!showSidebarUtilities) setNotificationsOpen(false)
  }, [showSidebarUtilities])
  const agentSection = activeDocumentId ?? 'home'
  const agentContextItems = [
    { section: 'home', label: '专家介绍', icon: House },
    { section: 'knowledge', label: '资料库', icon: Database },
    { section: 'skills', label: '技能', icon: WandSparkles },
    { section: 'mcp', label: '连接器', icon: Cable },
    { section: 'tools', label: '工具', icon: Wrench },
    { section: 'conversations', label: '工作记录', icon: ClipboardList },
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
        { view: 'settings' as const, label: '外观设置', icon: Settings },
        { view: 'settings-conversation' as const, label: '对话设置', icon: MessageCircleMore },
        { view: 'settings-shortcuts' as const, label: '快捷键', icon: Keyboard },
      ],
    },
    {
      label: '模型与知识',
      items: [
        { view: 'settings-models' as const, label: '模型服务', icon: Server },
        { view: 'settings-ai' as const, label: '助手设置', icon: Bot },
        { view: 'settings-yuxi' as const, label: '资料库服务', icon: Globe2 },
      ],
    },
    {
      label: '权限与数据',
      items: [
        { view: 'settings-projects' as const, label: '项目权限', icon: ShieldCheck },
        { view: 'settings-extensions' as const, label: '插件', icon: Wrench },
        { view: 'settings-usage' as const, label: '使用情况', icon: Activity },
        { view: 'maintenance' as const, label: '数据维护', icon: ShieldCheck },
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
    { view: 'knowledge-detail' as const, label: '文件详情', icon: BookOpen },
    { view: 'knowledge-graph' as const, label: '关系图', icon: Globe2 },
  ]

  const conversationList = useMemo(() => (
        <ScrollArea className="fox-sidebar-scroll fox-sidebar-group-scroll">
          {lifecycleView === 'active' && <section className="fox-sidebar-section">
            <div className="fox-section-head fox-sidebar-group-head"><span>项目</span><span><IconButton label="添加项目" onClick={onAddProject}><Plus size={14} /></IconButton><DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多项目操作"><MoreHorizontal size={14} /></Button></DropdownMenuTrigger><DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-project-menu"><DropdownMenuItem onSelect={onAddProject}><Plus />添加项目</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem disabled={!projectGroups.length} onSelect={() => setAllProjectsOpen(true)}><ChevronDown />全部展开</DropdownMenuItem><DropdownMenuItem disabled={!projectGroups.length} onSelect={() => setAllProjectsOpen(false)}><ChevronRight />全部收起</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem onSelect={() => onNavigate('settings-projects')}><ShieldCheck />项目权限</DropdownMenuItem></DropdownMenuContent></DropdownMenu></span></div>
            {projectGroups.length ? projectGroups.map((project) => {
              const open = projectOpen[project.key] ?? true
              return <div className="fox-project-group" key={project.key}>
                <ContextMenu>
                  <ContextMenuTrigger asChild>
                    {/* The project row is a container, not a selected row: a child
                        conversation being active must not light up its project
                        as well, or the sidebar shows two selections at once. */}
                    <SidebarMotionItem><div className="fox-project-row">
                      <button type="button" className="fox-project-row-main" title={`${project.name}\n${project.root}`} aria-expanded={open} onClick={() => setProjectOpen((current) => ({ ...current, [project.key]: !open }))}>
                        {open ? <FolderOpen size={15} /> : <Folder size={15} />}
                        <strong>{project.name}</strong>
                        {project.pinned && <Pin size={11} aria-label="已置顶项目" />}
                      </button>
                      <span className="fox-project-row-actions">
                        <IconButton label={`在 ${project.name} 中新建对话`} className="fox-project-add" onClick={() => onNewProjectChat(project.root)}><Plus size={13} /></IconButton>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label={`更多 ${project.name} 项目操作`}><MoreHorizontal size={13} /></Button></DropdownMenuTrigger>
                          <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-project-menu">
                            <AnimatedIconTrigger><DropdownMenuItem onSelect={() => onNewProjectChat(project.root)}><MessageCirclePlus />新建对话</DropdownMenuItem></AnimatedIconTrigger>
                            <DropdownMenuItem onSelect={() => onNavigate('settings-projects')}><ShieldCheck />项目权限</DropdownMenuItem>
                            <DropdownMenuItem disabled={!project.id} onSelect={() => onPinProject?.(project)}>{project.pinned ? <PinOff /> : <Pin />}{project.pinned ? '取消置顶' : '置顶项目'}</DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem variant="destructive" onSelect={() => onDeleteProject(project)}><Trash2 />删除项目</DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </span>
                    </div></SidebarMotionItem>
                  </ContextMenuTrigger>
                  <ContextMenuContent className="fox-project-menu">
                    <AnimatedIconTrigger><ContextMenuItem onSelect={() => onNewProjectChat(project.root)}><MessageCirclePlus />新建对话</ContextMenuItem></AnimatedIconTrigger>
                    <ContextMenuItem onSelect={() => onNavigate('settings-projects')}><ShieldCheck />项目权限</ContextMenuItem>
                    <ContextMenuItem disabled={!project.id} onSelect={() => onPinProject?.(project)}>{project.pinned ? <PinOff /> : <Pin />}{project.pinned ? '取消置顶' : '置顶项目'}</ContextMenuItem>
                    <ContextMenuSeparator />
                    <ContextMenuItem variant="destructive" onSelect={() => onDeleteProject(project)}><Trash2 />删除项目</ContextMenuItem>
                  </ContextMenuContent>
                </ContextMenu>
                {open && <div className="fox-project-children">{project.items.map((item) => (
                  <ContextMenu key={item.id}>
                    <ContextMenuTrigger asChild>
                      <SidebarMotionItem><div className={`fox-sidebar-tree-row fox-project-conversation-row ${item.active ? 'is-active' : ''}`}>
                        <button type="button" className="fox-sidebar-tree-main" title={item.title} onClick={() => onOpenConversation(item.id)}><ConversationRunStatus indicator={runIndicators?.get(item.id)} /><span>{item.title}</span></button>
                        <ConversationAttentionBadge indicator={runIndicators?.get(item.id)} />
                        <span className="fox-sidebar-row-time">{item.time}</span>
                        <span className="fox-sidebar-row-actions">
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多项目对话操作"><MoreHorizontal size={13} /></Button></DropdownMenuTrigger>
                            <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-conversation-menu">
                              <DropdownMenuItem onSelect={() => onPinConversation(item)}>{item.pinned ? <PinOff /> : <Pin />}{item.pinned ? '取消置顶' : '置顶对话'}</DropdownMenuItem>
                              <DropdownMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名</DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem onSelect={() => onArchiveConversation(item)}><Archive />归档对话</DropdownMenuItem>
                              <DropdownMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />删除对话</DropdownMenuItem>
                            </DropdownMenuContent>
                          </DropdownMenu>
                        </span>
                      </div></SidebarMotionItem>
                    </ContextMenuTrigger>
                    <ContextMenuContent className="fox-conversation-menu">
                      <ContextMenuItem onSelect={() => onPinConversation(item)}>{item.pinned ? <PinOff /> : <Pin />}{item.pinned ? '取消置顶' : '置顶对话'}</ContextMenuItem>
                      <ContextMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名</ContextMenuItem>
                      <ContextMenuSeparator />
                      <ContextMenuItem onSelect={() => onArchiveConversation(item)}><Archive />归档对话</ContextMenuItem>
                      <ContextMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />删除对话</ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                ))}</div>}
              </div>
            }) : <p className="fox-project-empty">暂无项目对话</p>}
          </section>}

          <section className="fox-sidebar-section fox-conversation-section">
            <div className="fox-section-head fox-sidebar-group-head">
              <span>{lifecycleView === 'archived' ? '已归档' : lifecycleView === 'trash' ? '回收站' : '对话'}</span>
              <span>
                {lifecycleView === 'active' && <IconButton label="新建对话" onClick={onNewChat}><Plus size={14} /></IconButton>}
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
              {sidebarDateGroups(visibleConversations).map((group) => <section className="fox-sidebar-date-group" key={group.key}><h3 className="fox-sidebar-date-heading"><MessageCircleMore size={14} /><span>{group.label}</span></h3>{group.items.map((item) => (
                <ContextMenu key={item.id}>
                  <ContextMenuTrigger asChild>
                    <SidebarMotionItem><div className={`fox-sidebar-tree-row ${item.active ? 'is-active' : ''}`}>
                      <button type="button" className="fox-sidebar-tree-main" title={item.title} onClick={() => onOpenConversation(item.id)}><ConversationRunStatus indicator={runIndicators?.get(item.id)} /><span>{item.title}</span></button>
                      <ConversationAttentionBadge indicator={runIndicators?.get(item.id)} />
                      <span className="fox-sidebar-row-time">{item.time}</span>
                      <span className="fox-sidebar-row-actions">
                        <DropdownMenu>
                          <Tooltip><TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多操作"><MoreHorizontal size={13} /></Button></DropdownMenuTrigger></TooltipTrigger><TooltipContent side="right"><p>更多操作</p></TooltipContent></Tooltip>
                          <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-conversation-menu">
                            {lifecycleView === 'active' ? <>
                              <DropdownMenuItem onSelect={() => onPinConversation(item)}>{item.pinned ? <PinOff /> : <Pin />}{item.pinned ? '取消置顶' : '置顶对话'}</DropdownMenuItem>
                              <DropdownMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名</DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem onSelect={() => onArchiveConversation(item)}><Archive />归档对话</DropdownMenuItem>
                              <DropdownMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />删除对话</DropdownMenuItem>
                            </> : lifecycleView === 'archived' ? <>
                              <DropdownMenuItem onSelect={() => onUnarchiveConversation(item)}><RotateCcw />取消归档</DropdownMenuItem>
                              <DropdownMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />删除对话</DropdownMenuItem>
                            </> : <>
                              <DropdownMenuItem onSelect={() => onRestoreConversation(item)}><RotateCcw />恢复对话</DropdownMenuItem>
                              <DropdownMenuSeparator />
                              <DropdownMenuItem variant="destructive" onSelect={() => onPurgeConversation(item)}><Trash2 />永久删除</DropdownMenuItem>
                            </>}
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </span>
                    </div></SidebarMotionItem>
                  </ContextMenuTrigger>
                  <ContextMenuContent className="fox-conversation-menu">
                  {lifecycleView === 'active' ? <>
                    <ContextMenuItem onSelect={() => onPinConversation(item)}>{item.pinned ? <PinOff /> : <Pin />}{item.pinned ? '取消置顶' : '置顶对话'}</ContextMenuItem>
                    <ContextMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名</ContextMenuItem>
                    <ContextMenuSeparator />
                    <ContextMenuItem onSelect={() => onArchiveConversation(item)}><Archive />归档对话</ContextMenuItem>
                    <ContextMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />删除对话</ContextMenuItem>
                  </> : lifecycleView === 'archived' ? <>
                    <ContextMenuItem onSelect={() => onUnarchiveConversation(item)}><RotateCcw />取消归档</ContextMenuItem>
                    <ContextMenuItem variant="destructive" onSelect={() => onTrashConversation(item)}><Trash2 />删除对话</ContextMenuItem>
                  </> : <>
                    <ContextMenuItem onSelect={() => onRestoreConversation(item)}><RotateCcw />恢复对话</ContextMenuItem>
                    <ContextMenuSeparator />
                    <ContextMenuItem variant="destructive" onSelect={() => onPurgeConversation(item)}><Trash2 />永久删除</ContextMenuItem>
                  </>}
                  </ContextMenuContent>
                </ContextMenu>
              ))}</section>)}
              {!visibleConversations.length && <p className="fox-project-empty">{lifecycleView === 'archived' ? '没有已归档的对话' : lifecycleView === 'trash' ? '回收站是空的' : '还没有对话，开始聊聊吧'}</p>}
            </div>
          </section>
        </ScrollArea>
  ), [lifecycleView, projectGroups, projectOpen, visibleConversations, runIndicators,
    onAddProject, onNewChat, onNewProjectChat, onNavigate, onPinProject, onDeleteProject,
    onOpenConversation, onPinConversation, onRenameConversation, onArchiveConversation,
    onTrashConversation, onUnarchiveConversation, onRestoreConversation, onPurgeConversation,
    setAllProjectsOpen])

  return (
    <SidebarMotion className={`fox-sidebar ${collapsed ? 'is-collapsed' : ''} ${!['chat', 'local-knowledge'].includes(sidebarMode) ? 'is-management' : ''} ${notificationsOpen ? 'is-notifications' : ''}`}>
      {/* Chat and 资料库 have nothing left in this strip once collapsed, and
          keeping a row-height block there would push the rail's first icon down
          away from the title bar. */}
      {!(collapsed && showSidebarUtilities) && <div className="fox-sidebar-title-safe">
        {!['chat', 'local-knowledge'].includes(sidebarMode) ? <button type="button" className="fox-management-back" aria-label="返回" title="返回" onClick={goBack}><ArrowLeft size={16} /><span>返回</span></button> : (collapsed ? null : <span className="fox-window-safe" />)}
      </div>}

      {collapsed && showSidebarUtilities && <div className="fox-sidebar-collapsed-rail">
        <Tooltip>
          <AnimatedIconTrigger><TooltipTrigger asChild><button type="button" className="fox-sidebar-mode-switch" aria-label={selectedMode === 'assistant' ? '切换到资料' : '切换到助手'} onClick={() => onModeChange(selectedMode === 'assistant' ? 'knowledge' : 'assistant')}>{selectedMode === 'assistant' ? <BookCopy /> : <Sparkles />}</button></TooltipTrigger></AnimatedIconTrigger>
          <TooltipContent side="right"><p>{selectedMode === 'assistant' ? '切换到资料' : '切换到助手'}</p></TooltipContent>
        </Tooltip>
        <AnimatedIconTrigger><button type="button" className="fox-sidebar-search-trigger" onClick={toggleSearch} aria-label="搜索" title={searchTitle}><Search size={16} /></button></AnimatedIconTrigger>
        <AnimatedIconTrigger><button type="button" className={`fox-sidebar-search-trigger ${notificationsOpen ? 'is-active' : ''}`} onClick={() => changeNotificationsOpen(!notificationsOpen)} aria-label="通知" title="通知" aria-pressed={notificationsOpen}><Bell size={16} /></button></AnimatedIconTrigger>
      </div>}

      {/* In management views the compact strip keeps only the way back: a
          full-height button would be taller than the strip it replaces. */}
      {collapsed && sidebarMode !== 'chat' && sidebarMode !== 'local-knowledge' && <div className="fox-sidebar-collapsed-rail is-management">
        <button type="button" className="fox-management-back fox-sidebar-collapsed-back" aria-label="返回" title="返回" onClick={goBack}><ArrowLeft size={16} /></button>
      </div>}

      {showSidebarUtilities && !collapsed && (
        <div className="fox-sidebar-mode-row">
          <SidebarModeTabs value={selectedMode} onValueChange={(value) => onModeChange(value as AssistantMode)}>
              <AnimatedIconTrigger><SidebarModeTab value="assistant"><Sparkles />助手</SidebarModeTab></AnimatedIconTrigger>
              {/* The trigger has to be wrapped, not the icon: AnimatedIconTrigger
                  injects its hover/focus handlers onto whatever it wraps, so an
                  icon-level wrapper leaves the label and padding as dead zones
                  and the animation only fires over the 14px glyph. */}
              <AnimatedIconTrigger><SidebarModeTab value="knowledge"><BookCopy />资料</SidebarModeTab></AnimatedIconTrigger>
            </SidebarModeTabs>
          <div className="fox-sidebar-mode-actions"><AnimatedIconTrigger><button type="button" className={`fox-sidebar-search-trigger ${searchOpen ? 'is-active' : ''}`} onClick={toggleSearch} aria-label="搜索" title={searchTitle}><Search /></button></AnimatedIconTrigger></div>
        </div>
      )}

      {sidebarMode === 'chat' && <div className="fox-sidebar-primary">
        <SidebarMotionItem><button className={`fox-sidebar-command is-emphasis ${newChatActive ? 'is-active' : ''}`} type="button" aria-label="新建对话" aria-current={newChatActive ? 'page' : undefined} onClick={onNewChat}><MessageCirclePlus size={18} /><span>新建对话</span>{newConversationShortcut && <kbd>{newConversationShortcut}</kbd>}</button></SidebarMotionItem>
        <SidebarMotionItem><button className={`fox-sidebar-command starts-destination-group ${agentViews.includes(activeView) ? 'is-active' : ''}`} type="button" aria-label="专家" aria-current={agentViews.includes(activeView) ? 'page' : undefined} onClick={() => onNavigate('agents')}><Users size={18} /><span>专家</span></button></SidebarMotionItem>
        <SidebarMotionItem><button className={`fox-sidebar-command ${activeView === 'plugins' ? 'is-active' : ''}`} type="button" aria-label="插件" aria-current={activeView === 'plugins' ? 'page' : undefined} onClick={() => onNavigate('plugins')}><Blocks size={18} /><span>插件</span></button></SidebarMotionItem>
      </div>}

      {!notificationsOpen && collapsed && sidebarMode === 'chat' && <SidebarCollapsedMenus projects={projectGroups} conversations={visibleConversations} indicators={runIndicators} onOpen={onOpenConversation} />}

      {sidebarMode === 'chat' ? <>{!notificationsOpen && (
        <div className="fox-sidebar-list-pane" hidden={collapsed}>
        {conversationList}
        </div>
      )}</> : sidebarMode === 'local-knowledge' ? <LocalKnowledgeSidebarNavigation collapsed={collapsed} activeView={activeView} activeEntityId={activeEntityId} activeDocumentId={activeDocumentId} navigate={onNavigate} hideLibrary={notificationsOpen} onCreate={() => { if (onCreateLocalKnowledge) onCreateLocalKnowledge(); else onNavigate('local-knowledge') }} /> : !notificationsOpen ? <>
        <div className={`fox-sidebar-primary fox-context-sidebar-nav ${sidebarMode === 'settings' ? 'is-settings' : ''}`}>
          {sidebarMode === 'agent'
            ? agentContextItems.map(({ section, label, icon: Icon }) => <SidebarMotionItem key={section}><button className={`fox-sidebar-command ${agentSection === section ? 'is-active' : ''}`} type="button" aria-label={label} aria-current={agentSection === section ? 'page' : undefined} onClick={() => onNavigate('agent-detail', activeEntityId ?? undefined, { documentId: section })}><Icon size={16} /><span>{label}</span></button></SidebarMotionItem>)
            : sidebarMode === 'settings'
              ? settingsContextGroups.map((group) => <section className="fox-context-sidebar-group" key={group.label}>{!collapsed && <span className="fox-context-sidebar-group-label">{group.label}</span>}{group.items.map(({ view, label, icon: Icon }) => <SidebarMotionItem key={view}><button className={`fox-sidebar-command ${activeSettingsView === view ? 'is-active' : ''}`} type="button" aria-label={label} aria-current={activeSettingsView === view ? 'page' : undefined} onClick={() => onNavigate(view)}><Icon size={18} /><span>{label}</span></button></SidebarMotionItem>)}</section>)
              : contextItems.map(({ view, label, icon: Icon }) => <SidebarMotionItem key={view}><button className={`fox-sidebar-command ${activeView === view ? 'is-active' : ''}`} type="button" aria-label={label} aria-current={activeView === view ? 'page' : undefined} onClick={() => onNavigate(view, activeEntityId ?? undefined)}><Icon size={18} /><span>{label}</span></button></SidebarMotionItem>)}
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
      <div className="fox-sidebar-footer">
        <div className="fox-sidebar-user-menu"><UserMenu
          user={{ name: profileName, email: profileDetail, avatarSrc: profile.avatar, plan: yuxiService?.status === 'connected' && yuxiUser ? '已连接资料库' : undefined }}
          open={profileMenuOpen}
          onOpenChange={setProfileMenuOpen}
          showName={!collapsed}
          align="start"
          triggerLabel={`打开用户菜单，${profileName}`}
          className="fox-sidebar-user-trigger"
          panelClassName="fox-sidebar-user-panel"
          showTheme={false}
          onThemeToggle={onTheme}
          items={[
            { label: '个人资料', icon: <UserRound size={16} />, onSelect: () => setProfileDialogOpen(true) },
            { label: '账户设置', icon: <Settings size={16} />, keys: settingsKeys, onSelect: onSettings },
          ]}
        /></div>
        {!collapsed && <div className="fox-sidebar-footer-actions">
          <AnimatedIconTrigger><button type="button" className="fox-icon-button" aria-label="切换主题" title="切换主题" onClick={onTheme}><SunMoon size={16} /></button></AnimatedIconTrigger>
          <AnimatedIconTrigger><button type="button" className="fox-icon-button" aria-label="设置" title="设置" onClick={onSettings}><Settings size={16} /></button></AnimatedIconTrigger>
        </div>}
      </div>
      <Dialog open={searchOpen} onOpenChange={changeSearchOpen}>
        <DialogContent
          className="fox-global-search-dialog fox-sidebar-search-dialog"
          showCloseButton={false}
          onCloseAutoFocus={(event) => {
            const target = searchReturnFocusRef.current
            searchReturnFocusRef.current = null
            if (!target || !target.isConnected) return
            event.preventDefault()
            target.focus()
          }}
        >
          <DialogHeader className="sr-only"><DialogTitle>搜索</DialogTitle><DialogDescription>搜索对话、项目、资料库、文件和任务。</DialogDescription></DialogHeader>
          <label className="fox-global-search-input-shell">
            <Search aria-hidden="true" />
            <input ref={searchInputRef} autoFocus value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索对话、项目、资料、文件或任务" />
            {search && <button type="button" onClick={() => setSearch('')} aria-label="清除搜索"><X /></button>}
          </label>
          {!normalizedSearch && <div className="fox-sidebar-search-idle" aria-live="polite"><FoxEmptyState
            title={runtimeConversations?.length ? '搜索 Fox' : '还没有对话'}
            description={runtimeConversations?.length ? '输入关键词搜索对话、项目、资料库、文件和任务。' : '开始一次新对话，或输入关键词搜索项目和资料。'}
            icon={runtimeConversations?.length ? <Search /> : <MessageCirclePlus />}
            variant={runtimeConversations?.length ? 'search' : 'empty'}
            action={!runtimeConversations?.length ? <button type="button" className="fox-sidebar-search-state-action" onClick={() => { changeSearchOpen(false); onNewChat() }}>新建对话</button> : undefined}
          /></div>}
          {normalizedSearch && <div className="fox-global-search-results" role="region" aria-label="全局搜索结果" aria-live="polite">
            {!searchError && searchResults === null && <FoxEmptyState title="正在搜索" description="正在查找对话、项目、资料和任务…" icon={<LoaderCircle />} variant="search" />}
            {searchError && <FoxEmptyState title="搜索失败" description={searchError} icon={<AlertTriangle />} variant="error" action={<button type="button" className="fox-sidebar-search-state-action" onClick={() => setSearchAttempt((attempt) => attempt + 1)}>重试</button>} />}
            {!searchError && searchResults?.length === 0 && <FoxEmptyState title="没有匹配的结果" description="换个关键词，搜索对话、项目、资料、文件或任务。" icon={<Search />} variant="search" action={<button type="button" className="fox-sidebar-search-state-action" onClick={() => { setSearch(''); searchInputRef.current?.focus() }}>清除关键词</button>} />}
            {!searchError && groupedSearchResults.map(([resultKind, records]) => <section key={resultKind}><header>{globalSearchKindLabel(resultKind)}<span>{records.length}</span></header>{records.map((item) => <button type="button" key={`${item.kind}:${item.id}`} onClick={() => openGlobalSearchResult(item)}><strong>{item.title}</strong><small>{item.subtitle}</small><time>{new Date(item.updatedAt).toLocaleDateString('zh-CN')}</time></button>)}</section>)}
          </div>}
        </DialogContent>
      </Dialog>
      <UserProfileDialog open={profileDialogOpen} onOpenChange={setProfileDialogOpen} profile={profile} detail={profileDetail} onSave={async (next) => { await saveProfile(next); toast.success('个人资料已更新') }} />
    </SidebarMotion>
  )
}

export const MemoizedSidebar = memo(Sidebar)
