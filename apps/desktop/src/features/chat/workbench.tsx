import { lazy, Suspense, useEffect, useMemo, useRef, useState, type ChangeEvent, type KeyboardEvent, type ReactNode } from 'react'
import {
  Activity,
  Archive,
  AlertTriangle,
  ArrowLeft,
  ArrowUp,
  BookOpen,
  Bot,
  Brain,
  Check,
  ChevronDown,
  ChevronRight,
  CircleStop,
  ClipboardList,
  Code2,
  Copy,
  ExternalLink,
  File,
  FileEdit,
  FilePlus2,
  FileText,
  Folder,
  FolderOpen,
  Folders,
  Globe2,
  House,
  CircleHelp,
  CircleDot,
  ImagePlus,
  Library,
  ListTodo,
  LoaderCircle,
  Maximize2,
  MessageCircleMore,
  MessageSquarePlus,
  Mic,
  Minus,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Pause,
  Paperclip,
  Play,
  Plus,
  Puzzle,
  RotateCcw,
  Search,
  Server,
  Settings,
  ShieldCheck,
  Sparkles,
  Square,
  Target,
  SunMoon,
  Terminal,
  ThumbsDown,
  ThumbsUp,
  Trash2,
  UserRound,
  Wrench,
  X
} from 'lucide-react'
import { createContext, useContext } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { flushSync } from 'react-dom'
import { toast } from 'sonner'
import { BorderBeam } from 'border-beam'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { Button } from '@/components/ui/button'
import { ShinyText } from '@/components/effects/shiny-text'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Sheet, SheetContent } from '@/components/ui/sheet'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import {
  Conversation,
  ConversationAutoScroll,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton
} from '@/components/ai-elements/conversation'
import {
  Message,
  MessageAction,
  MessageActions,
  MessageContent
} from '@/components/ai-elements/message'
import { Source, Sources, SourcesContent, SourcesTrigger } from '@/components/ai-elements/sources'
import {
  InlineCitation,
  InlineCitationCard,
  InlineCitationCardBody,
  InlineCitationCardTrigger,
  InlineCitationCarousel,
  InlineCitationCarouselContent,
  InlineCitationCarouselHeader,
  InlineCitationCarouselIndex,
  InlineCitationCarouselItem,
  InlineCitationCarouselNext,
  InlineCitationCarouselPrev,
  InlineCitationQuote,
  InlineCitationSource,
  InlineCitationText
} from '@/components/ai-elements/inline-citation'
import {
  Artifact,
  ArtifactAction,
  ArtifactActions,
  ArtifactContent,
  ArtifactDescription,
  ArtifactHeader,
  ArtifactTitle
} from '@/components/ai-elements/artifact'
import {
  ChainOfThought,
  ChainOfThoughtContent,
  ChainOfThoughtHeader,
  ChainOfThoughtSearchResult,
  ChainOfThoughtSearchResults,
  ChainOfThoughtStep
} from '@/components/ai-elements/chain-of-thought'
import { ToolInput as AIToolInput, ToolOutput as AIToolOutput } from '@/components/ai-elements/tool'
import {
  Confirmation,
  ConfirmationAction,
  ConfirmationActions,
  ConfirmationRejected,
  ConfirmationRequest,
  ConfirmationTitle
} from '@/components/ai-elements/confirmation'
import {
  Attachment,
  AttachmentInfo,
  AttachmentPreview,
  AttachmentRemove,
  Attachments
} from '@/components/ai-elements/attachments'
import { FileTree, FileTreeFile, FileTreeFolder } from '@/components/ai-elements/file-tree'
import { KnowledgeResourceExplorer } from '@/features/knowledge/knowledge-resource-explorer'
import { normalizeKnowledgeSourceLocator } from '@/features/knowledge/knowledge-source-locator'
import { Suggestion, Suggestions } from '@/components/ai-elements/suggestion'
import {
  Context as AIContext,
  ContextContent as AIContextContent,
  ContextContentBody as AIContextContentBody,
  ContextContentFooter as AIContextContentFooter,
  ContextContentHeader as AIContextContentHeader,
  ContextTrigger as AIContextTrigger
} from '@/components/ai-elements/context'
import {
  Queue,
  QueueItem,
  QueueItemActions,
  QueueItemContent,
  QueueItemDescription,
  QueueItemIndicator,
  QueueList
} from '@/components/ai-elements/queue'
import {
  Terminal as AITerminal,
  TerminalActions as AITerminalActions,
  TerminalContent as AITerminalContent,
  TerminalCopyButton as AITerminalCopyButton,
  TerminalHeader as AITerminalHeader,
  TerminalStatus as AITerminalStatus,
  TerminalTitle as AITerminalTitle
} from '@/components/ai-elements/terminal'
import {
  WebPreview,
  WebPreviewNavigation,
  WebPreviewNavigationButton,
  WebPreviewUrl
} from '@/components/ai-elements/web-preview'
import { Agent as AIAgent, AgentContent as AIAgentContent, AgentHeader as AIAgentHeader } from '@/components/ai-elements/agent'
import {
  PromptInput,
  PromptInputActionAddAttachments,
  PromptInputActionMenu,
  PromptInputActionMenuContent,
  PromptInputActionMenuItem,
  PromptInputActionMenuTrigger,
  PromptInputBody,
  PromptInputButton,
  PromptInputFooter,
  PromptInputSelect,
  PromptInputSelectContent,
  PromptInputSelectItem,
  PromptInputSelectTrigger,
  PromptInputSelectValue,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
  usePromptInputAttachments
} from '@/components/ai-elements/prompt-input'
import { citations } from './mock-data'
import { useYuxiService } from '@/features/settings/use-yuxi-service'
import { useYuxiUser } from '@/features/settings/use-yuxi-user'
import { useModelService } from '@/features/settings/use-model-service'
import { useYuxiModels } from '@/features/settings/use-yuxi-models'
import { useAgents } from '@/features/agents/use-agents'
import { useKnowledgeBases } from '@/features/knowledge/use-knowledge'
import { latestMessage, runIsActive, useDesktopConversation } from '@/features/conversations/hooks/use-desktop-conversation'
import { pendingRuntimeQuestion } from '@/features/conversations/model/pending-interactions'
import type { RuntimeQuestion, RuntimeQuestionRequest } from '@/features/conversations/model/pending-interactions'
import type { AgentRecord, ApprovalRecord, ArtifactRecord, AttachmentRecord, ConversationDetail, ConversationMessage, ConversationSummary, KnowledgeBaseRecord, KnowledgeBindingRecord, ModelServiceRecord, ProjectFileEntry, ProjectRecord, RunEventRecord, ToolCallRecord, YuxiModelRecord, YuxiUserRecord } from '@/features/conversations/model/types'
import type { KnowledgeSourceLocator, NavigateWorkspace, WorkspaceView } from '@/features/workspace/types'
import { idleMascots, mascotAt, mascotLibrary, workingMascots } from './mascot-library'
import type { MessageResponseProps } from '@/components/ai-elements/message-response'
import { UserProfileDialog, useUserProfile } from '@/features/profile/user-profile'

const AgentListPage = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentListPage })))
const AgentDetailPage = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentDetailPage })))
const KnowledgeListPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeListPage })))
const KnowledgeDetailPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeDetailPage })))
const KnowledgeGraphPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeGraphPage })))
const SettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.SettingsPage })))
const AiSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.AiSettingsPage })))
const ModelProvidersPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ModelProvidersPage })))
const YuxiSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.YuxiSettingsPage })))
const ProjectPermissionsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ProjectPermissionsPage })))
const ConversationSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ConversationSettingsPage })))
const UsageStatisticsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.UsageStatisticsPage })))
const ExtensionsSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ExtensionsSettingsPage })))
const AboutSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.AboutSettingsPage })))
const SkillsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.SkillsPage })))
const McpPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.McpPage })))
const MaintenancePage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.MaintenancePage })))
const ServicePage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ServicePage })))
const ModelServicePage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ModelServicePage })))
const LoginPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.LoginPage })))
const OnboardingPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.OnboardingPage })))
const MessageResponse = lazy(() => import('@/components/ai-elements/message-response').then((module) => ({ default: module.MessageResponse })))

function MarkdownResponse({ children, ...props }: MessageResponseProps) {
  return (
    <Suspense fallback={<div className="fox-markdown-loading">{typeof children === 'string' ? children : ''}</div>}>
      <MessageResponse {...props}>{children}</MessageResponse>
    </Suspense>
  )
}

const mascotAssets = {
  brand: '/mascot/fox/idle/fox_sit_nicely.png',
  idle: '/mascot/fox/idle/fox_sit_nicely.png',
  rest: '/mascot/fox/idle/fox_calm.png',
  sleep: '/mascot/fox/idle/fox_sleep_on_moon.png',
  work: '/mascot/fox/thinking/fox_think.png',
  welcome: '/mascot/fox/status/fox_sayhi.png',
  search: '/mascot/fox/thinking/fox_detective.png',
  laptop: '/mascot/fox/tools/fox_use_computer.png',
  wrench: '/mascot/fox/tools/fox_repair_hand_wrench.png',
  write: '/mascot/fox/thinking/fox_write_notes.png',
  files: '/mascot/fox/tools/fox_carry_files.png',
  checklist: '/mascot/fox/tools/fox_complete_checklist.png',
  cheer: '/mascot/fox/status/fox_celebrate.png',
  offline: '/mascot/fox/status/fox_worried.png'
} as const

const mascot = mascotAssets.idle
const layoutStorage = {
  sidebarCollapsed: 'fox.layout.sidebarCollapsed',
  sidebarWidth: 'fox.layout.sidebarWidth',
  rightMode: 'fox.layout.rightMode',
  rightSidebarCollapsed: 'fox.layout.rightSidebarCollapsed',
  rightWidth: 'fox.layout.rightWidth',
  theme: 'fox.theme',
  focusMode: 'fox.focusMode',
  terminalHeight: 'fox.layout.terminalHeight'
} as const

const terminalOutput = '\u001b[36mPS D:\\python\\projects\\Fox>\u001b[0m pnpm tauri dev\n\u001b[90mFox desktop is running on http://127.0.0.1:1421\u001b[0m'
const citationUrls = [
  'https://github.com/xingyuv/kun/blob/develop/LICENSE',
  'https://github.com/xingyuv/kun/blob/develop/CLA.md'
]

const licenseAnswerMarkdown = `### 企业内部使用

营利性企业将 Kun 用作内部效率工具、业务系统或其他商业运营用途，通常属于商业使用，**需要向版权持有者申请单独的书面商业授权**。

| 使用场景 | 是否允许 |
| --- | --- |
| 个人学习与非商业二次开发 | 允许 |
| 教育、公益研究 | 允许 |
| 营利企业内部业务使用 | 需要商业授权 |
| SaaS、商业产品集成或转售 | 需要商业授权 |`

type RightMode = 'todo' | 'changes' | 'browser' | 'files' | 'knowledge' | 'agents'
type ChatState = 'complete' | 'running' | 'question' | 'approval' | 'error' | 'denied'
type AssistantMode = 'assistant' | 'knowledge'
const settingsWorkspaceViews: WorkspaceView[] = ['settings', 'onboarding', 'settings-ai', 'settings-models', 'settings-yuxi', 'settings-projects', 'settings-conversation', 'settings-usage', 'settings-extensions', 'settings-about', 'model-service', 'service', 'skills', 'mcp', 'maintenance', 'login']

function knowledgeServiceDisplayName(name?: string | null) {
  const value = name?.trim()
  if (!value || /^yuxi(?:\s*(?:service|服务))?$/i.test(value)) return '知识库服务'
  return value
}

interface ConversationUsage {
  inputTokens: number
  outputTokens: number
  totalTokens: number
}

function isConfirmationQuestion(question: RuntimeQuestion) {
  if (question.options.length !== 2 || question.multiSelect) return false
  return question.options.every((option) => /确认|取消|同意|拒绝|继续|停止|是|否|yes|no/i.test(option.label))
}

function yuxiModelOverrideAllowed(agent?: AgentRecord | null) {
  if (!agent || agent.runtimeType !== 'yuxi') return false
  const items = agent.configurableItems
  if (!items || typeof items !== 'object' || Array.isArray(items)) return false
  const model = (items as Record<string, unknown>).model
  return Boolean(model && typeof model === 'object' && (model as Record<string, unknown>).kind === 'llm')
}
function latestConversationUsage(events: RunEventRecord[]): ConversationUsage {
  const usage = [...events].reverse().find((item) => item.eventType === 'usage.updated')?.event
  const inputTokens = typeof usage?.inputTokens === 'number' ? usage.inputTokens : 0
  const outputTokens = typeof usage?.outputTokens === 'number' ? usage.outputTokens : 0
  const reportedTotal = typeof usage?.totalTokens === 'number' ? usage.totalTokens : 0
  return { inputTokens, outputTokens, totalTokens: Math.max(reportedTotal, inputTokens + outputTokens) }
}

function compactTokenCount(value: number) {
  if (value < 1000) return String(value)
  const compact = value >= 10000 ? Math.round(value / 1000) : Math.round(value / 100) / 10
  return `${compact}k`
}

async function copyTextWithFeedback(text: string, successMessage: string) {
  try {
    if (!navigator.clipboard?.writeText) throw new Error('Clipboard API unavailable')
    await navigator.clipboard.writeText(text)
    toast.success(successMessage)
  } catch {
    toast.error('复制失败，请重试')
  }
}

function readStoredBoolean(key: string, fallback: boolean) {
  if (typeof window === 'undefined') return fallback
  const value = window.localStorage.getItem(key)
  return value === null ? fallback : value === '1'
}
function readStoredNumber(key: string, fallback: number, min: number, max: number) {
  if (typeof window === 'undefined') return fallback
  const value = Number(window.localStorage.getItem(key))
  return Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback
}

function readStoredRightMode(): RightMode | null {
  if (typeof window === 'undefined') return 'files'
  const value = window.localStorage.getItem(layoutStorage.rightMode)
  return value === 'todo' || value === 'changes' || value === 'browser' || value === 'files' || value === 'knowledge' || value === 'agents'
    ? value
    : value === 'closed' ? null : 'files'
}

function IconButton({
  label,
  children,
  onClick,
  active,
  className = '',
  tooltipSide = 'top'
}: {
  label: string
  children: ReactNode
  onClick?: () => void
  active?: boolean
  className?: string
  tooltipSide?: 'top' | 'right' | 'bottom' | 'left'
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={label}
          aria-pressed={active === undefined ? undefined : active}
          className={`fox-icon-button ${active ? 'is-active' : ''} ${className}`}
          onClick={onClick}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent side={tooltipSide}><p>{label}</p></TooltipContent>
    </Tooltip>
  )
}
function WorkMascot({ busy = false, size = 'sm' }: { busy?: boolean; size?: 'sm' | 'md' }) {
  return (
    <span className={`fox-work-mascot is-${size} ${busy ? 'is-busy' : ''}`} aria-hidden="true">
      <span className="fox-work-current" />
      <span className="fox-work-wave is-back" />
      <span className="fox-work-track">
        <span className="fox-work-body">
          <img src={busy ? mascotAssets.work : mascot} alt="" />
        </span>
      </span>
      <span className="fox-work-wave is-front" />
      <span className="fox-work-foam" />
      <span className="fox-work-bubbles" />
    </span>
  )
}

function WindowTitlebar({ onNewChat, onOpenProject, onSettings, onToggleSidebar, onToggleTerminal, onZoom }: {
  onNewChat: () => void
  onOpenProject: () => void
  onSettings: () => void
  onToggleSidebar: () => void
  onToggleTerminal: () => void
  onZoom: (delta: number) => void
}) {
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    try {
      const appWindow = getCurrentWindow()
      const syncMaximized = async () => {
        try {
          const value = await appWindow.isMaximized()
          if (!disposed) setMaximized(value)
        } catch {
          // Browser preview has no native window state.
        }
      }
      void syncMaximized()
      void appWindow.onResized(() => { void syncMaximized() }).then((cleanup) => { unlisten = cleanup })
    } catch {
      // Browser preview has no native window state.
    }
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  const windowAction = async (action: 'minimize' | 'maximize' | 'close') => {
    try {
      const appWindow = getCurrentWindow()
      if (action === 'minimize') await appWindow.minimize()
      if (action === 'maximize') {
        await appWindow.toggleMaximize()
        setMaximized(await appWindow.isMaximized())
      }
      if (action === 'close') await appWindow.close()
    } catch {
      // Browser preview has no native window; buttons remain harmless there.
    }
  }

  return (
    <header className="fox-window-titlebar" data-tauri-drag-region>
      <div className="fox-window-product" data-tauri-drag-region>
        <img src={mascotAssets.brand} alt="" />
        <strong>Fox</strong>
        <nav aria-label="应用菜单">
          <WindowMenu label="文件">
            <DropdownMenuItem onSelect={onNewChat}><MessageSquarePlus />新建对话<DropdownMenuShortcut>Ctrl N</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={onOpenProject}><FolderOpen />打开项目<DropdownMenuShortcut>Ctrl O</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={onSettings}><Settings />设置<DropdownMenuShortcut>Ctrl ,</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => void windowAction('close')}>退出</DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="编辑">
            <DropdownMenuItem onSelect={() => document.execCommand('undo')}>撤销<DropdownMenuShortcut>Ctrl Z</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={() => document.execCommand('redo')}>重做<DropdownMenuShortcut>Ctrl Y</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => document.execCommand('copy')}>复制<DropdownMenuShortcut>Ctrl C</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={() => document.execCommand('paste')}>粘贴<DropdownMenuShortcut>Ctrl V</DropdownMenuShortcut></DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="视图">
            <DropdownMenuItem onSelect={onToggleSidebar}><PanelLeftClose />切换侧边栏<DropdownMenuShortcut>Ctrl B</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={onToggleTerminal}><Terminal />切换终端<DropdownMenuShortcut>Ctrl J</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => onZoom(0.1)}>放大<DropdownMenuShortcut>Ctrl +</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onZoom(-0.1)}>缩小<DropdownMenuShortcut>Ctrl -</DropdownMenuShortcut></DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="帮助">
            <DropdownMenuItem onSelect={() => toast('Fox 文档将在帮助中心开放')}><BookOpen />Fox 文档</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => toast('Fox 0.1.0 · 通用桌面智能体')}><Sparkles />关于 Fox</DropdownMenuItem>
          </WindowMenu>
        </nav>
      </div>
      <div className="fox-window-controls">
        <button type="button" aria-label="最小化" onClick={() => void windowAction('minimize')}><Minus /></button>
        <button type="button" aria-label={maximized ? '还原窗口' : '最大化'} title={maximized ? '还原窗口' : '最大化'} onClick={() => void windowAction('maximize')}>{maximized ? <Copy /> : <Square />}</button>
        <button type="button" className="is-close" aria-label="关闭" onClick={() => void windowAction('close')}><X /></button>
      </div>
    </header>
  )
}

function WindowMenu({ label, children }: { label: string; children: ReactNode }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild><button type="button">{label}</button></DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={3} className="fox-window-menu-content">{children}</DropdownMenuContent>
    </DropdownMenu>
  )
}

function Sidebar({
  collapsed,
  assistantMode,
  onCollapse,
  onModeChange,
  onNewChat,
  onNewProjectChat,
  onAddProject,
  onOpenConversation,
  onRenameConversation,
  onDeleteConversation,
  onNavigate,
  onExitManagement,
  onTheme,
  onSettings,
  runtimeConversations,
  onSearchConversations,
  activeConversationId,
  yuxiService,
  yuxiUser,
  activeView,
  activeEntityId,
  activeDocumentId,
}: {
  collapsed: boolean
  assistantMode: AssistantMode
  onCollapse: () => void
  onModeChange: (mode: AssistantMode) => void
  onNewChat: () => void
  onNewProjectChat: (projectRoot?: string) => void
  onAddProject: () => void
  onOpenConversation: (conversationId?: string) => void
  onRenameConversation: (conversation: { id: string; title: string }) => void
  onDeleteConversation: (conversation: { id: string; title: string }) => void
  onNavigate: NavigateWorkspace
  onExitManagement: () => void
  onTheme: () => void
  onSettings: () => void
  runtimeConversations?: ConversationSummary[]
  onSearchConversations?: (query: string) => Promise<ConversationSummary[]>
  activeConversationId?: string
  yuxiService?: { name: string; status: string; connectionType: 'local' | 'lan' | 'remote' } | null
  yuxiUser?: YuxiUserRecord | null
  activeView: WorkspaceView
  activeEntityId?: string | null
  activeDocumentId?: string | null
}) {
  const [projectOpen, setProjectOpen] = useState<Record<string, boolean>>({})
  const [searchOpen, setSearchOpen] = useState(false)
  const [search, setSearch] = useState('')
  const [searchResults, setSearchResults] = useState<ConversationSummary[] | null>(null)
  const [profileMenuOpen, setProfileMenuOpen] = useState(false)
  const [profileDialogOpen, setProfileDialogOpen] = useState(false)
  const searchInputRef = useRef<HTMLInputElement | null>(null)
  const normalizedSearch = search.trim().toLocaleLowerCase()
  useEffect(() => {
    if (!normalizedSearch || !onSearchConversations) {
      setSearchResults(null)
      return
    }
    setSearchResults(null)
    let cancelled = false
    const timer = window.setTimeout(() => {
      void onSearchConversations(search.trim()).then((records) => {
        if (!cancelled) setSearchResults(records)
      })
    }, 180)
    return () => {
      cancelled = true
      window.clearTimeout(timer)
    }
  }, [normalizedSearch, onSearchConversations, search])
  const conversationItems = (searchResults ?? runtimeConversations)?.map((item) => ({
    id: item.id,
    title: item.title,
    projectRoot: item.projectRoot,
    time: item.lastMessageAt ? new Date(item.lastMessageAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '',
    active: item.id === activeConversationId
  })) ?? []
  const projectConversationItems = conversationItems.filter((item) => Boolean(item.projectRoot))
  const regularConversationItems = conversationItems.filter((item) => !item.projectRoot)
  const visibleProjectConversations = projectConversationItems.filter((item) =>
    !normalizedSearch || searchResults !== null || item.title.toLocaleLowerCase().includes(normalizedSearch) || item.projectRoot?.toLocaleLowerCase().includes(normalizedSearch)
  )
  const projectGroups = Array.from(visibleProjectConversations.reduce((groups, item) => {
    const projectRoot = item.projectRoot?.trim()
    if (!projectRoot) return groups
    const normalizedRoot = projectRoot.replace(/[\\/]+$/, '').replace(/\//g, '\\').toLocaleLowerCase()
    const existing = groups.get(normalizedRoot)
    if (existing) existing.items.push(item)
    else groups.set(normalizedRoot, {
      key: normalizedRoot,
      root: projectRoot.replace(/[\\/]+$/, ''),
      name: projectRoot.replace(/[\\/]+$/, '').split(/[\\/]/).at(-1) || projectRoot,
      items: [item],
    })
    return groups
  }, new Map<string, { key: string; root: string; name: string; items: typeof visibleProjectConversations }>()).values())
  const visibleConversations = regularConversationItems.filter((item) =>
    !normalizedSearch || searchResults !== null || item.title.toLocaleLowerCase().includes(normalizedSearch)
  )
  const toggleSearch = () => {
    setSearchOpen((open) => {
      const next = !open
      if (next) window.requestAnimationFrame(() => searchInputRef.current?.focus())
      return next
    })
  }
  const setAllProjectsOpen = (open: boolean) => {
    setProjectOpen(Object.fromEntries(projectGroups.map((project) => [project.key, open])))
  }
  const { profile, save: saveProfile } = useUserProfile({ name: yuxiUser?.username ?? 'Hury', avatar: yuxiUser?.avatar })
  const profileName = profile.name
  const profileInitial = profile.initial
  const profileDetail = yuxiUser
    ? [yuxiUser.departmentName, yuxiUser.role].filter(Boolean).join(' · ')
    : 'Fox 本地用户'
  const agentViews: WorkspaceView[] = ['agents', 'agent-detail']
  const knowledgeViews: WorkspaceView[] = ['knowledge', 'knowledge-detail', 'knowledge-graph']
  const knowledgeDetailViews: WorkspaceView[] = ['knowledge-detail', 'knowledge-graph']
  const sidebarMode = settingsWorkspaceViews.includes(activeView)
    ? 'settings'
    : activeView === 'agent-detail'
      ? 'agent'
    : knowledgeDetailViews.includes(activeView)
      ? 'knowledge'
      : 'chat'
  const agentSection = activeDocumentId ?? 'home'
  const agentContextItems = [
    { section: 'home', label: '首页', icon: House },
    { section: 'conversations', label: '对话任务', icon: ClipboardList },
    { section: 'memory', label: '记忆', icon: Brain },
    { section: 'skills', label: 'Skills', icon: Puzzle },
    { section: 'tools', label: '工具', icon: Wrench },
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
  const contextItems = sidebarMode === 'settings'
    ? [
        { view: 'settings' as const, label: '应用', icon: Settings },
        { view: 'settings-models' as const, label: '模型供应商', icon: Server },
        { view: 'settings-ai' as const, label: '模型偏好', icon: Bot },
        { view: 'settings-yuxi' as const, label: '知识库服务', icon: Globe2 },
        { view: 'settings-projects' as const, label: '项目与权限', icon: ShieldCheck },
        { view: 'settings-conversation' as const, label: '输入与对话', icon: MessageCircleMore },
        { view: 'settings-usage' as const, label: '使用统计', icon: Activity },
        { view: 'settings-extensions' as const, label: '扩展', icon: Wrench },
        { view: 'maintenance' as const, label: '数据与诊断', icon: ShieldCheck },
        { view: 'settings-about' as const, label: '关于与更新', icon: CircleHelp },
      ]
    : [
        { view: 'knowledge-detail' as const, label: '文件与详情', icon: BookOpen },
        { view: 'knowledge-graph' as const, label: '知识图谱', icon: Globe2 },
      ]

  return (
    <aside className={`fox-sidebar ${collapsed ? 'is-collapsed' : ''} ${sidebarMode !== 'chat' ? 'is-management' : ''}`}>
      <div className="fox-sidebar-title-safe">
        {collapsed ? <IconButton label="展开侧边栏" tooltipSide="bottom" onClick={onCollapse}><PanelLeftOpen size={16} /></IconButton> : sidebarMode !== 'chat' ? <button type="button" className="fox-management-back" onClick={sidebarMode === 'agent' ? () => onNavigate('agents') : onExitManagement}><ArrowLeft size={16} /><span>返回</span></button> : <span className="fox-window-safe" />}
      </div>

      {sidebarMode === 'chat' && !collapsed && (
        <Tabs value={assistantMode} onValueChange={(value) => onModeChange(value as AssistantMode)} className="fox-mode-tabs">
          <TabsList>
            <TabsTrigger value="assistant"><Sparkles />助手</TabsTrigger>
            <TabsTrigger value="knowledge"><Library />知识</TabsTrigger>
          </TabsList>
        </Tabs>
      )}

      {sidebarMode === 'chat' ? <><div className="fox-sidebar-primary">
        <button className="fox-sidebar-command is-emphasis" onClick={onNewChat}><MessageSquarePlus size={16} /><span>新建对话</span><kbd>Ctrl N</kbd></button>
        <button className={`fox-sidebar-command ${searchOpen ? 'is-active' : ''}`} onClick={toggleSearch}><Search size={16} /><span>搜索</span><kbd>Ctrl K</kbd></button>
        {!collapsed && searchOpen && <label className="fox-sidebar-search fox-sidebar-global-search">
          <Search size={14} />
          <input ref={searchInputRef} value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索项目和对话" />
          {search && <button type="button" onClick={() => setSearch('')} aria-label="清除"><X size={13} /></button>}
        </label>}
        <button className={`fox-sidebar-command ${agentViews.includes(activeView) ? 'is-active' : ''}`} onClick={() => onNavigate('agents')}><Bot size={16} /><span>智能体</span></button>
        <button className={`fox-sidebar-command ${knowledgeViews.includes(activeView) ? 'is-active' : ''}`} onClick={() => onNavigate('knowledge')}><Library size={16} /><span>知识库</span></button>
      </div>

      {!collapsed && (
        <ScrollArea className="fox-sidebar-scroll">
          <section className="fox-sidebar-section">
            <div className="fox-section-head"><span>项目</span><span><IconButton label="添加项目" onClick={onAddProject}><Plus size={14} /></IconButton><DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多项目操作"><MoreHorizontal size={14} /></Button></DropdownMenuTrigger><DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-project-menu"><DropdownMenuItem onSelect={onAddProject}><Plus />添加项目</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem disabled={!projectGroups.length} onSelect={() => setAllProjectsOpen(true)}><ChevronDown />全部展开</DropdownMenuItem><DropdownMenuItem disabled={!projectGroups.length} onSelect={() => setAllProjectsOpen(false)}><ChevronRight />全部收起</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem onSelect={() => onNavigate('settings-projects')}><ShieldCheck />项目与权限</DropdownMenuItem></DropdownMenuContent></DropdownMenu></span></div>
            {projectGroups.length ? projectGroups.map((project) => {
              const open = projectOpen[project.key] ?? true
              return <div className="fox-project-group" key={project.key}>
                <div className={`fox-project-row ${project.items.some((item) => item.active) ? 'is-active' : ''}`}>
                  <button type="button" className="fox-project-row-main" title={project.root} onClick={() => setProjectOpen((current) => ({ ...current, [project.key]: !open }))}>
                    {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                    <FolderOpen size={15} />
                    <strong>{project.name}</strong>
                  </button>
                  <IconButton label={`在 ${project.name} 中新建对话`} className="fox-project-add" onClick={() => onNewProjectChat(project.root)}><Plus size={13} /></IconButton>
                  <small>{project.items.length}</small>
                </div>
                {open && <div className="fox-project-children">{project.items.map((item) => <button type="button" key={item.id} className={`fox-project-conversation ${item.active ? 'is-active' : ''}`} onClick={() => onOpenConversation(item.id)}><FileText size={14} /><span>{item.title}</span><small>{item.time}</small></button>)}</div>}
              </div>
            }) : <p className="fox-project-empty">暂无项目对话</p>}
          </section>

          <section className="fox-sidebar-section fox-conversation-section">
            <div className="fox-section-head"><span>对话</span><span><IconButton label="新建会话" onClick={onNewChat}><Plus size={14} /></IconButton><IconButton label="归档会话"><Archive size={14} /></IconButton></span></div>
            <div className="fox-conversation-list">
              {visibleConversations.map((item) => (
                <div key={item.id} className={`fox-sidebar-tree-row ${item.active ? 'is-active' : ''}`}>
                  <button type="button" className="fox-sidebar-tree-main" onClick={() => onOpenConversation(item.id)}><MessageCircleMore size={14} /><span>{item.title}</span></button>
                  <span className="fox-sidebar-row-time">{item.time}</span>
                  <span className="fox-sidebar-row-actions">
                    <DropdownMenu>
                      <Tooltip><TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多会话操作"><MoreHorizontal size={13} /></Button></DropdownMenuTrigger></TooltipTrigger><TooltipContent side="right"><p>更多会话操作</p></TooltipContent></Tooltip>
                      <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-conversation-menu">
                        <DropdownMenuItem onSelect={() => onRenameConversation(item)}><FileEdit />重命名</DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem variant="destructive" onSelect={() => onDeleteConversation(item)}><Trash2 />删除对话</DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </span>
                </div>
              ))}
            </div>
          </section>
        </ScrollArea>
      )}</> : <>
        <div className={`fox-sidebar-primary fox-context-sidebar-nav ${sidebarMode === 'settings' ? 'is-settings' : ''}`}>
          {sidebarMode === 'agent'
            ? agentContextItems.map(({ section, label, icon: Icon }) => <button key={section} className={`fox-sidebar-command ${agentSection === section ? 'is-active' : ''}`} onClick={() => onNavigate('agent-detail', activeEntityId ?? undefined, { documentId: section })}><Icon size={16} /><span>{label}</span></button>)
            : contextItems.map(({ view, label, icon: Icon }) => <button key={view} className={`fox-sidebar-command ${(sidebarMode === 'settings' ? activeSettingsView : activeView) === view ? 'is-active' : ''}`} onClick={() => onNavigate(view, view === 'knowledge-detail' || view === 'knowledge-graph' ? activeEntityId ?? undefined : undefined)}><Icon size={16} /><span>{label}</span></button>)}
        </div>
        {!collapsed && sidebarMode === 'knowledge' && (
          <KnowledgeResourceExplorer
            knowledgeId={activeEntityId}
            selectedDocumentId={activeDocumentId}
            navigate={onNavigate}
          />
        )}
      </>}

      <div className="fox-sidebar-spacer" />
      <div className="fox-sidebar-footer">
        <DropdownMenu open={profileMenuOpen} onOpenChange={setProfileMenuOpen}>
          <Tooltip>
            <TooltipTrigger asChild>
              <DropdownMenuTrigger asChild>
                <button type="button" className="fox-profile" aria-label="打开用户菜单">
                  <Avatar><AvatarImage src={profile.avatar} alt="" /><AvatarFallback>{profileInitial}</AvatarFallback></Avatar>
                  <span className="fox-profile-copy"><strong>{profileName}</strong><small><i className={yuxiUser ? '' : 'is-muted'} />{yuxiUser ? '知识库已登录' : yuxiService?.status === 'connected' ? '知识库已连接 · 未登录' : 'Fox 本地用户'}</small></span>
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
            <div className="fox-profile-menu-status"><i className={yuxiUser ? '' : 'is-muted'} /><span className="fox-profile-menu-copy"><strong>{knowledgeServiceDisplayName(yuxiService?.name)}</strong><small>{yuxiService?.status === 'connected' ? `${yuxiService.connectionType === 'local' ? '本地' : yuxiService.connectionType === 'lan' ? '局域网' : '远程'}服务 · ${yuxiUser ? '账户已登录' : '账户未登录'}` : '服务未连接，Fox 本地助手仍可使用'}</small></span></div>
          </DropdownMenuContent>
        </DropdownMenu>
        <IconButton label="切换主题" onClick={onTheme}><SunMoon size={16} /></IconButton>
        <IconButton label="设置" onClick={onSettings}><Settings size={16} /></IconButton>
      </div>
      <UserProfileDialog open={profileDialogOpen} onOpenChange={setProfileDialogOpen} profile={profile} detail={profileDetail} onSave={async (next) => { await saveProfile(next); toast.success('个人资料已更新') }} />
    </aside>
  )
}

function AssistantProcess({ running = false }: { running?: boolean }) {
  const [open, setOpen] = useState(false)

  return (
    <ChainOfThought open={open} onOpenChange={setOpen} className="fox-chain-of-thought">
      <ChainOfThoughtHeader className="fox-chain-of-thought-header">
        {running ? <ShinyText text="正在处理" speed={2.05} className="fox-runtime-status-text" /> : <span>工作过程（7 步）</span>}
        {!running && <small>· 思考 10 秒</small>}
      </ChainOfThoughtHeader>
      <ChainOfThoughtContent className="fox-chain-of-thought-content">
        <ChainOfThoughtStep icon={Search} label="查找 LICENSE*" status="complete">
          <ChainOfThoughtSearchResults>
            <ChainOfThoughtSearchResult>Kun/LICENSE</ChainOfThoughtSearchResult>
          </ChainOfThoughtSearchResults>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep icon={Search} label="搜索 license、License、LICENSE" status="complete">
          <ChainOfThoughtSearchResults>
            <ChainOfThoughtSearchResult>Kun/CLA.md</ChainOfThoughtSearchResult>
            <ChainOfThoughtSearchResult>Kun/README.md</ChainOfThoughtSearchResult>
          </ChainOfThoughtSearchResults>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep
          icon={Brain}
          label="分析许可证问题"
          description="需要依据仓库中的 LICENSE 与 CLA 判断二次开发和企业内部使用范围。"
          status="complete"
        />
        <ChainOfThoughtStep
          icon={MessageCircleMore}
          label="好的，我来查一下 Fox 项目（Kun）的许可证信息。"
          status="complete"
        />
        <ChainOfThoughtStep icon={FileText} label="读取 Kun/LICENSE" status="complete">
          <div className="fox-chain-detail">确认项目采用 PolyForm Noncommercial License 1.0.0。</div>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep
          icon={Brain}
          label="核对许可范围"
          description="非商业学习、研究和实验允许使用；商业用途需要单独书面授权。"
          status="complete"
        />
        <ChainOfThoughtStep
          icon={ShieldCheck}
          label={running ? '正在判断企业内部使用场景…' : '判断企业内部使用场景'}
          description="营利性企业用于内部业务通常属于商业用途，需要向版权持有者申请授权。"
          status={running ? 'active' : 'complete'}
        />
      </ChainOfThoughtContent>
    </ChainOfThought>
  )
}

function ApprovalPrompt({ onApprove, onDeny }: { onApprove: () => void; onDeny: () => void }) {
  return (
    <Confirmation approval={{ id: 'fox-write-file' }} state="approval-requested" className="fox-confirmation">
      <ConfirmationRequest>
        <div className="fox-confirmation-body">
          <span className="fox-prompt-figure"><img src={mascotAssets.wrench} alt="" /></span>
          <div><ConfirmationTitle>允许 Fox 修改本地文件？</ConfirmationTitle><p><code>apps/desktop/src/styles/workbench.css</code></p><small>Fox 将写入此工作区中的一个文件。你可以在 Changes 面板检查修改。</small></div>
        </div>
        <ConfirmationActions className="fox-confirmation-actions">
          <ConfirmationAction variant="ghost" onClick={onDeny}>拒绝</ConfirmationAction>
          <ConfirmationAction onClick={onApprove}>允许一次</ConfirmationAction>
        </ConfirmationActions>
      </ConfirmationRequest>
    </Confirmation>
  )
}

function RuntimeApprovalPrompt({ approval, onResolve }: { approval: ApprovalRecord; onResolve: (approvalId: string, approved: boolean) => void | Promise<boolean> }) {
  const request = approval.request
  const isCommand = approval.toolName === 'run_command'
  const [submitting, setSubmitting] = useState(false)
  const submittingRef = useRef(false)
  const resolve = async (approved: boolean) => {
    if (submittingRef.current) return
    submittingRef.current = true
    setSubmitting(true)
    const resolved = await onResolve(approval.id, approved)
    if (resolved === false) {
      submittingRef.current = false
      setSubmitting(false)
    }
  }
  return (
    <Confirmation approval={{ id: approval.id }} state="approval-requested" className="fox-confirmation fox-runtime-confirmation">
      <ConfirmationRequest>
        <div className="fox-confirmation-body">
          <span className="fox-prompt-figure"><img src={isCommand ? mascotAssets.laptop : mascotAssets.wrench} alt="" /></span>
          <div><ConfirmationTitle>{request.title ?? '允许 Fox 执行此操作？'}</ConfirmationTitle><p><code>{request.target ?? request.cwd ?? approval.toolName}</code></p><small>{request.summary ?? approval.requestedAction}</small></div>
        </div>
        {request.command && <div className="fox-approval-command"><Terminal size={13} /><code>{request.command}</code></div>}
        {request.diff && <pre className="fox-approval-diff"><code>{request.diff}</code></pre>}
        <ConfirmationActions className="fox-confirmation-actions">
          <ConfirmationAction variant="ghost" disabled={submitting} onClick={() => void resolve(false)}>拒绝</ConfirmationAction>
          <ConfirmationAction disabled={submitting} onClick={() => void resolve(true)}>{submitting ? '处理中…' : '允许一次'}</ConfirmationAction>
        </ConfirmationActions>
      </ConfirmationRequest>
    </Confirmation>
  )
}

function DeniedPrompt() {
  return (
    <Confirmation approval={{ id: 'fox-write-file', approved: false }} state="approval-responded" className="fox-denied-confirmation">
      <ConfirmationRejected>
        <div className="fox-denied-prompt">
          <img className="fox-inline-mascot" src={mascotAssets.cheer} alt="" />
          <span><strong>操作已取消</strong><small>Fox 没有修改任何本地文件。</small></span>
        </div>
      </ConfirmationRejected>
    </Confirmation>
  )
}

function errorPresentation(error?: string | null) {
  const detail = error?.trim() ?? ''
  if (/Pi Runtime dependencies|Fox Runtime|runtime (?:script|executable)|Node\.js is required/i.test(detail)) {
    return {
      title: 'Fox 本地运行时不可用',
      description: '本地智能体运行环境未能启动。请确认 Fox 依赖完整，并在重启应用后重试。',
      detail,
    }
  }
  if (/knowledge|知识库|yuxi|ECONNREFUSED.*5010/i.test(detail)) {
    return {
      title: '无法连接到知识库服务',
      description: '知识库服务暂时没有响应。请确认服务已启动，或稍后重试。',
      detail,
    }
  }
  return {
    title: 'Fox 暂时无法完成请求',
    description: '运行过程中遇到了问题。你可以重试，或在设置中查看运行诊断。',
    detail,
  }
}

function ErrorPrompt({ onRetry, error }: { onRetry: () => void; error?: string | null }) {
  const presentation = errorPresentation(error)
  return (
    <div className="fox-error-prompt" role="alert">
      <span className="fox-prompt-figure"><img src={mascotAssets.offline} alt="" /></span>
      <div><strong>{presentation.title}</strong><p>{presentation.description}</p>{presentation.detail && <small>{presentation.detail}</small>}</div>
      <Button size="sm" variant="outline" onClick={onRetry}><RotateCcw size={14} />重试</Button>
    </div>
  )
}

const clarificationOptions = [
  { id: 'analyze', title: '仅分析', detail: '先阅读相关文件并给出建议，不写入工作区' },
  { id: 'edit', title: '直接修改', detail: '按推荐方案修改文件，写入前仍会请求授权' },
  { id: 'backup', title: '先备份再修改', detail: '保留原文件副本后再执行修改' }
] as const

function QuestionPrompt({ onAnswer }: { onAnswer: (answer: string) => void }) {
  return (
    <div className="fox-question-prompt">
      <div className="fox-question-heading">
        <span className="fox-prompt-figure"><img src={mascotAssets.search} alt="" /></span>
        <div><strong>你希望我怎样处理这些文件？</strong><p>确认执行方式后，Fox 会继续当前任务。</p></div>
      </div>
      <div className="fox-question-options">
        {clarificationOptions.map((option, index) => (
          <button type="button" key={option.id} onClick={() => onAnswer(option.title)}>
            <kbd>{index + 1}</kbd><span><strong>{option.title}</strong><small>{option.detail}</small></span><ChevronRight size={14} />
          </button>
        ))}
      </div>
    </div>
  )
}

function LicenseInlineCitation() {
  return (
    <InlineCitation className="fox-inline-citation">
      <InlineCitationText>Kun 使用 PolyForm Noncommercial License 1.0.0</InlineCitationText>
      <InlineCitationCard>
        <InlineCitationCardTrigger sources={citationUrls} className="fox-inline-citation-trigger">2 个依据</InlineCitationCardTrigger>
        <InlineCitationCardBody className="fox-inline-citation-card">
          <InlineCitationCarousel>
            <InlineCitationCarouselHeader className="fox-inline-citation-head">
              <InlineCitationCarouselPrev />
              <InlineCitationCarouselIndex />
              <InlineCitationCarouselNext />
            </InlineCitationCarouselHeader>
            <InlineCitationCarouselContent>
              <InlineCitationCarouselItem>
                <InlineCitationSource title="PolyForm Noncommercial License 1.0.0" url="Kun/LICENSE" description="项目当前使用的非商业许可协议。" />
                <InlineCitationQuote>许可范围涵盖个人、教育、研究及其他非商业用途。</InlineCitationQuote>
              </InlineCitationCarouselItem>
              <InlineCitationCarouselItem>
                <InlineCitationSource title="Contributor License Agreement" url="Kun/CLA.md" description="确认项目授权与商业许可安排。" />
                <InlineCitationQuote>商业使用需要获得版权持有者单独的书面授权。</InlineCitationQuote>
              </InlineCitationCarouselItem>
            </InlineCitationCarouselContent>
          </InlineCitationCarousel>
        </InlineCitationCardBody>
      </InlineCitationCard>
    </InlineCitation>
  )
}

type RuntimeToolStep = {
  id: string
  seq: number
  name: string
  input: unknown
  output?: unknown
  isError: boolean
  completed: boolean
  awaitingUser: boolean
}

type RuntimeReasoningStep = {
  id: string
  seq: number
  label: string
  detail: string
}

type RuntimeSource = {
  id: string
  title: string
  knowledgeBaseId: string
  documentId: string
  excerpt: string
  locator: KnowledgeSourceLocator
}

function compactValue(value: unknown, fallback = '') {
  if (typeof value === 'string') return value.trim()
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  return fallback
}

function toolActivity(tool: RuntimeToolStep) {
  const input = tool.input && typeof tool.input === 'object' ? tool.input as Record<string, unknown> : {}
  const path = compactValue(input.path || input.file || input.directory || input.cwd)
  const query = compactValue(input.pattern || input.query || input.search || input.url)
  const subject = path || query
  const verb = tool.completed ? '已完成' : '正在调用'

  if (tool.name === 'ask_user_question') return tool.awaitingUser ? '等待你的回答' : 'Fox 正在向你提问'

  if (tool.name === 'read') return `${tool.completed ? '已读取' : 'Fox 正在读取'}${path ? ` ${path}` : '文件'}`
  if (tool.name === 'ls') return `${tool.completed ? '已查看' : 'Fox 正在查看'}${path ? ` ${path}` : '文件夹'}`
  if (tool.name === 'find') return `${tool.completed ? '已查找' : 'Fox 正在查找'}${query ? ` ${query}` : '文件'}${path ? ` · ${path}` : ''}`
  if (tool.name === 'grep') return `${tool.completed ? '已搜索' : 'Fox 正在搜索'}${query ? ` ${query}` : '文件内容'}${path ? ` · ${path}` : ''}`
  if (/web|browser|search/i.test(tool.name)) return `${tool.completed ? '已搜索' : 'Fox 正在搜索'}${subject ? ` ${subject}` : '网页'}`
  return `${tool.completed ? verb : `Fox ${verb}`} ${tool.name}${subject ? ` · ${subject}` : ''}`
}

function reasoningLabel(text: string, index: number, total: number) {
  const normalized = text.toLowerCase()
  if (/final|respond|response|answer|consolidat|summar|organize|整理|总结|回复/.test(normalized)) return '组织最终回答'
  if (/issue|problem|bug|risk|edge case|potential|permission|error|问题|风险|异常/.test(normalized)) return '评估潜在问题'
  if (/inspect|check|review|examine|read|look|verify|检查|读取|确认/.test(normalized)) return '检查现有实现'
  if (/improve|suggest|recommend|optimi|改进|优化|建议/.test(normalized)) return '整理改进方案'
  if (index === 0) return '理解任务'
  if (index === total - 1) return '形成处理结论'
  return `分析任务 ${index + 1}`
}

function splitReasoningSteps(text: string, prefix: string) {
  const paragraphs = text.split(/\n\s*\n+/).map((item) => item.trim()).filter(Boolean)
  if (paragraphs.length === 0) return []

  const maxSteps = 4
  const targetLength = Math.max(420, Math.ceil(text.length / Math.min(maxSteps, paragraphs.length)))
  const groups: string[] = []
  let current = ''
  for (const paragraph of paragraphs) {
    if (current && current.length + paragraph.length > targetLength && groups.length < maxSteps - 1) {
      groups.push(current)
      current = paragraph
    } else {
      current = current ? `${current}\n\n${paragraph}` : paragraph
    }
  }
  if (current) groups.push(current)

  return groups.map((detail, index) => ({
    id: `${prefix}-${index}`,
    seq: Number(prefix.split('-').at(-1) ?? 0) + index / 100,
    label: reasoningLabel(detail, index, groups.length),
    detail,
  }))
}

function runtimeProcess(events: RunEventRecord[]) {
  const reasoningGroups: Array<{ source: string; text: string; seq: number; lastSeq: number }> = []
  const tools = new Map<string, RuntimeToolStep>()
  const sources = new Map<string, RuntimeSource>()
  for (const item of events) {
    if (item.eventType === 'reasoning.delta' && typeof item.event.delta === 'string') {
      const source = typeof item.event.source === 'string' ? item.event.source : 'provider'
      const previous = reasoningGroups.at(-1)
      if (previous?.source === source && item.seq === previous.lastSeq + 1) {
        previous.text += item.event.delta
        previous.lastSeq = item.seq
      } else {
        reasoningGroups.push({ source, text: item.event.delta, seq: item.seq, lastSeq: item.seq })
      }
      continue
    }
    if (item.eventType === 'source.added') {
      const id = compactValue(item.event.id, `source-${item.seq}`)
      sources.set(id, {
        id,
        title: compactValue(item.event.title, '知识库来源'),
        knowledgeBaseId: compactValue(item.event.knowledgeBaseId),
        documentId: compactValue(item.event.documentId),
        excerpt: compactValue(item.event.excerpt),
        locator: normalizeKnowledgeSourceLocator(item.event, id),
      })
      continue
    }
    if (!item.eventType.startsWith('tool.')) continue
    const toolCallId = typeof item.event.toolCallId === 'string' ? item.event.toolCallId : `tool-${item.seq}`
    const current = tools.get(toolCallId) ?? {
      id: toolCallId,
      seq: item.seq,
      name: typeof item.event.tool === 'string' ? item.event.tool : 'tool',
      input: item.event.input ?? {},
      isError: false,
      completed: false,
      awaitingUser: false,
    }
    if (item.event.input !== undefined) current.input = item.event.input
    if (item.eventType === 'tool.completed') {
      current.output = item.event.result
      current.isError = item.event.isError === true
      current.completed = true
      current.awaitingUser = Boolean(
        item.event.result
        && typeof item.event.result === 'object'
        && (item.event.result as Record<string, unknown>).status === 'awaiting_user'
      )
    }
    tools.set(toolCallId, current)
  }
  const reasoning = reasoningGroups.flatMap((group) => splitReasoningSteps(group.text, `reasoning-${group.seq}`))
  const timeline = [
    ...reasoning.map((step) => ({ kind: 'reasoning' as const, seq: step.seq, step })),
    ...[...tools.values()].map((tool) => ({ kind: 'tool' as const, seq: tool.seq, tool })),
  ].sort((left, right) => left.seq - right.seq)
  return { reasoning, tools: [...tools.values()], sources: [...sources.values()], timeline }
}

function RuntimeSources({ sources, knowledgeBindings = [], onOpenSource }: { sources: RuntimeSource[]; knowledgeBindings?: KnowledgeBindingRecord[]; onOpenSource?: (source: RuntimeSource) => void }) {
  const [selectedSource, setSelectedSource] = useState<RuntimeSource | null>(null)
  const selectedKnowledgeName = selectedSource
    ? knowledgeBindings.find((binding) => binding.knowledgeBaseId === selectedSource.knowledgeBaseId)?.knowledgeBaseName || selectedSource.knowledgeBaseId || '知识库'
    : '知识库'
  if (!sources.length) return null
  return <>
    <Sources className="fox-sources fox-runtime-sources"><SourcesTrigger count={sources.length}>引用了 {sources.length} 个知识库来源<ChevronDown size={14} /></SourcesTrigger><SourcesContent>{sources.map((source) => <Source key={source.id} href="#" title={source.title} onClick={(event) => { event.preventDefault(); setSelectedSource(source) }}><BookOpen size={14} /><span><strong>{source.title}</strong>{source.excerpt && <small>{source.excerpt}</small>}</span><ChevronRight size={14} /></Source>)}</SourcesContent></Sources>
    <Dialog open={Boolean(selectedSource)} onOpenChange={(open) => { if (!open) setSelectedSource(null) }}>
      <DialogContent className="fox-source-dialog">
        <DialogHeader>
          <div className="fox-source-dialog-heading"><span><BookOpen size={18} /></span><div><DialogTitle>{selectedSource?.title ?? '知识库引用'}</DialogTitle><DialogDescription>模型回答所引用的知识片段</DialogDescription></div></div>
        </DialogHeader>
        <div className="fox-source-dialog-meta">
          <span><Library size={14} />{selectedKnowledgeName}</span>
          <span><FileText size={14} />{selectedSource?.title ?? '原始文件'}</span>
          {selectedSource?.locator.page && <span>第 {selectedSource.locator.page} 页</span>}
        </div>
        <div className="fox-source-dialog-excerpt">{selectedSource?.excerpt || '该引用暂未返回可展示的文本片段。'}</div>
        <DialogFooter>
          <Button variant="outline" onClick={() => selectedSource?.excerpt && void copyTextWithFeedback(selectedSource.excerpt, '已复制引用内容')} disabled={!selectedSource?.excerpt}><Copy size={14} />复制引用</Button>
          <Button disabled={!selectedSource?.knowledgeBaseId || !selectedSource?.documentId || !onOpenSource} onClick={() => { if (!selectedSource || !onOpenSource) return; onOpenSource(selectedSource); setSelectedSource(null) }}><ExternalLink size={14} />查看原文</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}

function RuntimeArtifacts({ artifacts }: { artifacts: ArtifactRecord[] }) {
  if (!artifacts.length) return null
  return <div className="fox-message-artifacts" aria-label="回复产物">
    {artifacts.map((artifact) => {
      const isWeb = artifact.mediaType === 'text/html' || /html|web/i.test(artifact.artifactType)
      const ArtifactIcon = isWeb ? Globe2 : artifact.mediaType?.startsWith('image/') ? ImagePlus : FileText
      return <Artifact key={artifact.id} className="fox-message-artifact">
        <ArtifactHeader className="fox-message-artifact-head">
          <div className="fox-message-artifact-title">
            <span className="fox-message-artifact-icon"><ArtifactIcon size={15} /></span>
            <div><ArtifactTitle>{artifact.displayName}</ArtifactTitle><ArtifactDescription>{artifact.status === 'completed' ? formatFileSize(artifact.byteSize) : artifact.status}</ArtifactDescription></div>
          </div>
          <ArtifactActions><ArtifactAction icon={isWeb ? Globe2 : ExternalLink} tooltip={isWeb ? '在预览中打开' : '打开产物'} onClick={() => toast('产物打开能力将在文件工作区接入后启用')} /></ArtifactActions>
        </ArtifactHeader>
      </Artifact>
    })}
  </div>
}

function RuntimeAssistantMessage({ message, processEvents, running, artifacts, assistantName, modelName, knowledgeBindings, onOpenSource }: { message: ConversationMessage; processEvents: RunEventRecord[]; running: boolean; artifacts: ArtifactRecord[]; assistantName: string; modelName?: string; knowledgeBindings?: KnowledgeBindingRecord[]; onOpenSource?: (source: RuntimeSource) => void }) {
  const parsed = splitAssistantContent(message.content ?? '')
  const completed = message.status === 'completed' && !running
  const replyTime = new Date(message.updatedAt || message.createdAt).toLocaleString([], { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })

  return (
    <div className="fox-turn-anchor">
      <Message from="assistant" className="fox-message fox-assistant-message">
        <MessageContent className="fox-assistant-content">
          <RuntimeProcess events={processEvents} running={running} />
          {parsed.answer && <div className="fox-answer-body"><MarkdownResponse className="fox-answer-response">{parsed.answer}</MarkdownResponse></div>}
          <RuntimeArtifacts artifacts={artifacts} />
          <RuntimeSources sources={runtimeProcess(processEvents).sources} knowledgeBindings={knowledgeBindings} onOpenSource={onOpenSource} />
        </MessageContent>
        <div className="fox-message-footer">
          <MessageActions className="fox-message-actions">
            <MessageAction tooltip="复制" onClick={() => parsed.answer ? void copyTextWithFeedback(parsed.answer, '已复制回复') : toast('暂无可复制内容')}><Copy size={14} /></MessageAction>
            <MessageAction tooltip="有帮助" onClick={() => toast.success('感谢反馈')}><ThumbsUp size={14} /></MessageAction>
            <MessageAction tooltip="没有帮助" onClick={() => toast('已记录反馈')}><ThumbsDown size={14} /></MessageAction>
          </MessageActions>
          <div className="fox-message-meta"><span>{assistantName}</span>{modelName && modelName !== assistantName && <span>{modelName}</span>}<span>{replyTime}</span>{!completed && <span className="fox-message-live-meta">处理中</span>}</div>
        </div>
      </Message>
    </div>
  )
}

function RuntimeReasoningItem({ step, running }: { step: RuntimeReasoningStep; running: boolean }) {
  return (
    <ChainOfThoughtStep icon={Brain} label={<RuntimeStepDetail label={step.label} active={running}><div className="fox-chain-detail"><MarkdownResponse>{step.detail}</MarkdownResponse></div></RuntimeStepDetail>} status={running ? 'active' : 'complete'} />
  )
}

function RuntimeToolItem({ tool }: { tool: RuntimeToolStep }) {
  const ToolIcon = tool.isError
    ? AlertTriangle
    : tool.awaitingUser
      ? CircleHelp
    : tool.name === 'read'
      ? FileText
      : tool.name === 'ls' || tool.name === 'find'
        ? FolderOpen
        : /grep|web|browser|search/i.test(tool.name)
          ? Search
          : CircleDot
  return (
    <ChainOfThoughtStep icon={ToolIcon} label={
      <RuntimeStepDetail label={toolActivity(tool)} active={!tool.completed} defaultOpen={tool.isError}>
        <AIToolInput className="fox-runtime-tool-detail" input={tool.input as never} />
        {tool.completed && <AIToolOutput className="fox-runtime-tool-detail" output={tool.output as never} errorText={tool.isError ? '工具执行失败' : undefined} />}
      </RuntimeStepDetail>
    } status={tool.awaitingUser ? 'pending' : !tool.completed ? 'active' : 'complete'} />
  )
}

function RuntimeStepDetail({ label, active = false, defaultOpen = false, children }: { label: string; active?: boolean; defaultOpen?: boolean; children: ReactNode }) {
  return (
    <Collapsible defaultOpen={defaultOpen} className="fox-runtime-step-detail">
      <CollapsibleTrigger className="fox-runtime-step-trigger">
        {active ? <ShinyText text={label} speed={2.05} className="fox-runtime-step-shiny" /> : <span>{label}</span>}{active && <small>处理中</small>}<ChevronRight />
      </CollapsibleTrigger>
      <CollapsibleContent className="fox-runtime-step-content">{children}</CollapsibleContent>
    </Collapsible>
  )
}

function RuntimeProcess({ events, running }: { events: RunEventRecord[]; running: boolean }) {
  const [open, setOpen] = useState(false)
  const process = runtimeProcess(events)
  const terminalEvent = [...events].reverse().find((item) => [
    'run.completed', 'run.cancelled', 'run.failed', 'run.interrupted',
  ].includes(item.eventType))
  const hasLifecycle = running || Boolean(terminalEvent) || events.some((item) => (
    item.eventType === 'run.started' || item.eventType === 'message.started'
  ))
  const lifecycleOnly = process.timeline.length === 0 && hasLifecycle
  const stepCount = lifecycleOnly ? 1 : process.timeline.length
  const latestStep = process.timeline.at(-1)
  const activeTool = latestStep?.kind === 'tool' && !latestStep.tool.completed ? latestStep.tool : undefined
  const lifecycleFailed = terminalEvent?.eventType === 'run.failed' || terminalEvent?.eventType === 'run.interrupted'
  const lifecycleCancelled = terminalEvent?.eventType === 'run.cancelled'
  const awaitingUser = terminalEvent?.eventType === 'run.completed'
    && terminalEvent.event.completionReason === 'awaiting_user'
  const lifecycleLabel = running
    ? '正在理解你的请求'
    : awaitingUser
      ? '等待你的回答'
    : lifecycleFailed
      ? '处理请求时遇到问题'
      : lifecycleCancelled
        ? '请求已取消'
        : '已完成请求处理'
  const processTitle = activeTool
    ? toolActivity(activeTool)
    : running
      ? latestStep?.kind === 'reasoning' ? `Fox ${latestStep.step.label}` : 'Fox 正在思考'
      : awaitingUser
        ? '等待你的回答'
      : lifecycleFailed
        ? '工作过程未完成'
        : lifecycleCancelled
          ? '工作过程已取消'
          : '工作过程已完成'

  if (stepCount === 0 && !running) return null
  return (
    <ChainOfThought open={open} onOpenChange={setOpen} className="fox-chain-of-thought fox-runtime-process">
      <ChainOfThoughtHeader className="fox-chain-of-thought-header">
        <span className={`fox-runtime-process-summary ${running ? 'is-running' : ''}`}>
          <WorkMascot busy={running} />
          <small className="fox-runtime-step-count" aria-label={`${stepCount} 个步骤`}>{stepCount}</small>
          {running ? <ShinyText text={processTitle} speed={2.05} className="fox-runtime-status-text" /> : <span>{processTitle}</span>}
        </span>
      </ChainOfThoughtHeader>
      <ChainOfThoughtContent className="fox-chain-of-thought-content">
        {process.timeline.length > 0 ? process.timeline.map((item, index) => item.kind === 'reasoning'
          ? <RuntimeReasoningItem key={item.step.id} step={item.step} running={running && index === process.timeline.length - 1 && !activeTool} />
          : <RuntimeToolItem key={item.tool.id} tool={item.tool} />
        ) : <ChainOfThoughtStep
          className={`fox-runtime-empty-step ${running ? 'is-running' : 'is-complete'}`}
          icon={running ? LoaderCircle : awaitingUser ? CircleHelp : lifecycleFailed ? AlertTriangle : Check}
          label={lifecycleLabel}
          status={running ? 'active' : awaitingUser ? 'pending' : 'complete'}
        />}
      </ChainOfThoughtContent>
    </ChainOfThought>
  )
}

function splitAssistantContent(content: string) {
  const reasoning: string[] = []
  const answer = content.replace(/<think>([\s\S]*?)<\/think>|<think>([\s\S]*)$/g, (_match, closed: string | undefined, open: string | undefined) => {
    const value = (closed ?? open ?? '').trim()
    if (value) reasoning.push(value)
    return ''
  }).trim()
  return { answer, reasoning: reasoning.join('\n\n') }
}

function SentMessageAttachments({ attachments }: { attachments: AttachmentRecord[] }) {
  if (attachments.length === 0) return null
  return <Attachments variant="inline" className="fox-message-attachments">
    {attachments.map((attachment) => (
      <Attachment key={attachment.id} data={{
        id: attachment.id,
        type: 'file',
        filename: attachment.displayName,
        mediaType: attachment.mediaType ?? 'application/octet-stream',
        url: '',
      }}>
        <AttachmentPreview />
        <AttachmentInfo />
      </Attachment>
    ))}
  </Attachments>
}

function RuntimeTimeline({ messages, attachments, artifacts, pendingMessage, events, approvals, activeRunId, activeRunModel, runtimeRunning, state, streamingText, runtimeError, assistantName = 'Fox 默认助手', hasEarlierMessages, loadingEarlierMessages, onLoadEarlierMessages, onRetry, onResolveApproval, onOpenSource }: { messages: ConversationMessage[]; attachments: AttachmentRecord[]; artifacts: ArtifactRecord[]; pendingMessage?: ConversationMessage | null; events: RunEventRecord[]; approvals: ApprovalRecord[]; activeRunId?: string; activeRunModel?: string; runtimeRunning: boolean; state: ChatState; streamingText: string; runtimeError?: string | null; assistantName?: string; hasEarlierMessages?: boolean; loadingEarlierMessages?: boolean; onLoadEarlierMessages?: () => void; onRetry: () => void; onResolveApproval: (approvalId: string, approved: boolean) => void | Promise<boolean>; onOpenSource?: (source: RuntimeSource) => void }) {
  const runtimeContext = useContext(ComposerRuntimeContext)
  const resolvedAssistantName = runtimeContext.assistantName ?? assistantName
  const resolvedRunModel = runtimeContext.runModel ?? activeRunModel
  const storedMessages = messages.filter((message) => message.role === 'user' || message.role === 'assistant')
  const pendingAlreadyStored = Boolean(pendingMessage && storedMessages.some((message) =>
    message.role === 'user' && message.content === pendingMessage.content && message.createdAt >= pendingMessage.createdAt - 3000
  ))
  const visibleMessages = pendingMessage && !pendingAlreadyStored ? [...storedMessages, pendingMessage] : [...storedMessages]
  const currentRunAssistant = activeRunId
    ? visibleMessages.find((message) => message.role === 'assistant' && message.runId === activeRunId)
    : undefined
  const currentRunHasProcess = Boolean(activeRunId && events.some((item) => item.runId === activeRunId))
  if (activeRunId && !currentRunAssistant && (runtimeRunning || Boolean(streamingText) || currentRunHasProcess)) {
    const latestOrdinal = visibleMessages.reduce((highest, message) => Math.max(highest, message.ordinal), 0)
    const now = Date.now()
    visibleMessages.push({
      id: `assistant-${activeRunId}`,
      conversationId: visibleMessages.at(-1)?.conversationId ?? 'pending',
      runId: activeRunId,
      role: 'assistant',
      kind: 'text',
      content: streamingText,
      status: runtimeRunning ? 'streaming' : 'completed',
      ordinal: latestOrdinal + 1,
      createdAt: now,
      updatedAt: now,
    })
  }
  const latestUserMessageId = [...visibleMessages].reverse().find((message) => message.role === 'user')?.id
  const currentAssistantActivity = activeRunId
    ? visibleMessages.find((message) => message.role === 'assistant' && message.runId === activeRunId)
    : undefined
  const currentRunLastSeq = activeRunId
    ? events.reduce((highest, item) => item.runId === activeRunId ? Math.max(highest, item.seq) : highest, 0)
    : 0
  const autoScrollKey = runtimeRunning
    ? [latestUserMessageId, activeRunId, currentRunLastSeq > 0 || currentAssistantActivity ? 'active' : 'waiting'].join(':')
    : latestUserMessageId
  const pendingApprovals = approvals.filter((approval) => approval.status === 'pending')
  return (
    <Conversation className="fox-conversation">
      <ConversationAutoScroll scrollKey={autoScrollKey} />
      <ConversationTurnRail messages={visibleMessages} artifacts={artifacts} />
      <ConversationContent className="fox-conversation-content" scrollClassName="fox-conversation-scroll">
        {hasEarlierMessages && <div className="fox-history-loader"><Button variant="ghost" size="sm" disabled={loadingEarlierMessages} onClick={onLoadEarlierMessages}>{loadingEarlierMessages && <LoaderCircle className="animate-spin" />}加载更早消息</Button></div>}
        <div className="fox-thread-date"><span>今天</span></div>
        {visibleMessages.map((message) => {
          if (message.role === 'user') {
            const messageAttachments = attachments.filter((attachment) => attachment.messageId === message.id)
            return <div id={`fox-turn-${message.id}`} key={message.id} className="fox-turn-anchor" data-turn-message-id={message.id}><Message from="user" className="fox-message fox-user-message"><MessageContent className="fox-user-bubble">{message.content}</MessageContent><SentMessageAttachments attachments={messageAttachments} /><div className="fox-user-meta"><span>Fox · {message.status === 'completed' ? '已发送' : message.status === 'sending' ? '发送中' : message.status}</span><span><MessageAction tooltip="复制" onClick={() => void copyTextWithFeedback(message.content, '已复制消息')}><Copy size={13} /></MessageAction></span></div></Message></div>
          }
          const isCurrentRun = Boolean(activeRunId && message.runId === activeRunId)
          const rawContent = isCurrentRun ? streamingText || message.content : message.content
          const parsed = splitAssistantContent(rawContent)
          const storedProcessEvents = message.runId ? events.filter((item) => item.runId === message.runId) : []
          const hasStoredReasoning = storedProcessEvents.some((item) => item.eventType === 'reasoning.delta')
          const processEvents = parsed.reasoning && !hasStoredReasoning
            ? [...storedProcessEvents, {
                runId: message.runId ?? `history-${message.id}`,
                seq: storedProcessEvents.reduce((highest, item) => Math.max(highest, item.seq), 0) + 1,
                eventType: 'reasoning.delta',
                event: { type: 'reasoning.delta', delta: parsed.reasoning, source: 'yuxi-history' },
                createdAt: message.updatedAt,
              }]
            : storedProcessEvents
          const running = isCurrentRun && runtimeRunning
          const messageArtifacts = artifacts.filter((artifact) => Boolean(message.runId) && artifact.runId === message.runId)
          const eventModel = processEvents.find((item) => item.eventType === 'run.started' && typeof item.event.model === 'string')?.event.model as string | undefined
          return <RuntimeAssistantMessage key={`assistant-turn-${message.runId ?? message.id}`} message={{ ...message, content: rawContent }} processEvents={processEvents} running={running} artifacts={messageArtifacts} assistantName={resolvedAssistantName} modelName={eventModel ?? (isCurrentRun ? resolvedRunModel : undefined)} knowledgeBindings={runtimeContext.knowledgeBindings} onOpenSource={onOpenSource ?? runtimeContext.onOpenSource} />
        })}
        {pendingApprovals.map((approval) => <div key={approval.id} className="fox-turn-anchor"><Message from="assistant" className="fox-message fox-assistant-message"><MessageContent className="fox-assistant-content"><RuntimeApprovalPrompt approval={approval} onResolve={onResolveApproval} /></MessageContent></Message></div>)}
        {state === 'error' && <div className="fox-turn-anchor"><Message from="assistant" className="fox-message fox-assistant-message"><MessageContent className="fox-assistant-content"><ErrorPrompt onRetry={onRetry} error={runtimeError} /></MessageContent></Message></div>}
      </ConversationContent>
      <ConversationScrollButton className="fox-scroll-button" />
    </Conversation>
  )
}

function Timeline({ empty, state, prompt, mascotSrc, mascotActive = false, runtimeMessages, runtimeAttachments, runtimeArtifacts, pendingMessage, runtimeEvents, runtimeApprovals, runtimeRunId, runtimeRunModel, runtimeRunning = false, runtimeReply, runtimeError, assistantName, hasEarlierMessages, loadingEarlierMessages, onLoadEarlierMessages, onApprove, onDeny, onRetry, onAnswer, onStart, onResolveApproval, onOpenSource }: { empty: boolean; state: ChatState; prompt: string; mascotSrc?: string; mascotActive?: boolean; runtimeMessages?: ConversationMessage[]; runtimeAttachments?: AttachmentRecord[]; runtimeArtifacts?: ArtifactRecord[]; pendingMessage?: ConversationMessage | null; runtimeEvents?: RunEventRecord[]; runtimeApprovals?: ApprovalRecord[]; runtimeRunId?: string; runtimeRunModel?: string; runtimeRunning?: boolean; runtimeReply?: string; runtimeError?: string | null; assistantName?: string; hasEarlierMessages?: boolean; loadingEarlierMessages?: boolean; onLoadEarlierMessages?: () => void; onApprove: () => void; onDeny: () => void; onRetry: () => void; onAnswer: (answer: string) => void; onStart: (suggestion: string) => void; onResolveApproval: (approvalId: string, approved: boolean) => void | Promise<boolean>; onOpenSource?: (source: RuntimeSource) => void }) {
  const running = state === 'running'

  if (empty) {
    return (
        <Conversation className="fox-conversation">
        <ConversationEmptyState className="fox-empty-conversation">
          <div className="fox-empty-heading">
            {mascotSrc && <div className={`fox-empty-mascot ${mascotActive ? 'is-active' : ''}`} aria-hidden="true"><img key={mascotSrc} src={mascotSrc} alt="" /></div>}
            <div className="fox-empty-heading-copy"><h2>今天想一起做点什么？</h2><p>Fox 可以和你对话、处理本地文件，也可以从知识库中检索资料。</p></div>
          </div>
          <Suggestions className="fox-starter-suggestions">
            <Suggestion suggestion="整理这个项目中的文档" onClick={onStart} />
            <Suggestion suggestion="从知识库查找相关资料" onClick={onStart} />
            <Suggestion suggestion="读取一个文件并帮我分析" onClick={onStart} />
            <Suggestion suggestion="帮我规划并完成一个任务" onClick={onStart} />
          </Suggestions>
        </ConversationEmptyState>
      </Conversation>
    )
  }
  if (runtimeMessages) {
    return <RuntimeTimeline messages={runtimeMessages} attachments={runtimeAttachments ?? []} artifacts={runtimeArtifacts ?? []} pendingMessage={pendingMessage} events={runtimeEvents ?? []} approvals={runtimeApprovals ?? []} activeRunId={runtimeRunId} activeRunModel={runtimeRunModel} runtimeRunning={runtimeRunning} state={state} streamingText={runtimeReply ?? ''} runtimeError={runtimeError} assistantName={assistantName} hasEarlierMessages={hasEarlierMessages} loadingEarlierMessages={loadingEarlierMessages} onLoadEarlierMessages={onLoadEarlierMessages} onRetry={onRetry} onResolveApproval={onResolveApproval} onOpenSource={onOpenSource} />
  }
  return (
    <Conversation className="fox-conversation">
      <ConversationContent className="fox-conversation-content" scrollClassName="fox-conversation-scroll">
        <div className="fox-thread-date"><span>今天</span></div>
        <div className="fox-turn-anchor">
        <Message from="user" className="fox-message fox-user-message">
          <MessageContent className="fox-user-bubble">{prompt}</MessageContent>
          <div className="fox-user-meta"><span>DeepSeek V3.2 · 刚刚</span><span><MessageAction tooltip="复制"><Copy size={13} /></MessageAction><MessageAction tooltip="编辑"><FileEdit size={13} /></MessageAction></span></div>
        </Message>
        </div>
        <div className="fox-turn-anchor">
        <Message from="assistant" className="fox-message fox-assistant-message">
          <MessageContent className="fox-assistant-content">
            {(state === 'running' || state === 'complete') && <AssistantProcess running={running} />}
            {state === 'question' ? <QuestionPrompt onAnswer={onAnswer} /> : state === 'approval' ? <ApprovalPrompt onApprove={onApprove} onDeny={onDeny} /> : state === 'error' ? <ErrorPrompt onRetry={onRetry} /> : state === 'denied' ? <DeniedPrompt /> : running ? (
              <div className="fox-live-response"><WorkMascot busy /><ShinyText text="正在连接知识库并整理结果" speed={2.05} className="fox-runtime-status-text" /><i><b /><b /><b /></i></div>
            ) : <>
              <div className="fox-answer-body">
                <h2>Kun 的许可证</h2>
                <p><LicenseInlineCitation />，允许个人学习、研究、实验和其他非商业用途，也可以在许可范围内进行二次开发。</p>
                <MarkdownResponse className="fox-answer-response">{licenseAnswerMarkdown}</MarkdownResponse>
              </div>
              <Sources defaultOpen className="fox-sources">
              <SourcesTrigger count={citations.length}>引用了 {citations.length} 个来源<ChevronDown size={14} /></SourcesTrigger>
              <SourcesContent>
                {citations.map((item, index) => (
                  <Source key={item.id} href="#" title={item.title} onClick={(event) => event.preventDefault()}>
                    <span className="fox-citation-index">{index + 1}</span>
                    <span><strong>{item.title}</strong><small>{item.detail}</small></span>
                    <ChevronRight size={14} />
                  </Source>
                ))}
              </SourcesContent>
              </Sources>
            </>}
          </MessageContent>
          {state === 'complete' && <MessageActions className="fox-message-actions">
            <MessageAction tooltip="复制" onClick={() => { void navigator.clipboard?.writeText('Kun 使用 PolyForm Noncommercial License 1.0.0，企业商业使用需要单独授权。'); toast.success('已复制回复') }}><Copy size={14} /></MessageAction>
            <MessageAction tooltip="有帮助" onClick={() => toast.success('感谢反馈')}><ThumbsUp size={14} /></MessageAction>
            <MessageAction tooltip="没有帮助" onClick={() => toast('已记录反馈')}><ThumbsDown size={14} /></MessageAction>
            <MessageAction tooltip="重新生成" onClick={() => toast('重新生成将在接入知识库服务后启用')}><RotateCcw size={14} /></MessageAction>
          </MessageActions>}
        </Message>
        </div>
      </ConversationContent>
      <ConversationScrollButton className="fox-scroll-button" />
    </Conversation>
  )
}

function ConversationTurnRail({ messages, artifacts }: { messages: ConversationMessage[]; artifacts: ArtifactRecord[] }) {
  const [hoveredId, setHoveredId] = useState<string | null>(null)
  const [activeId, setActiveId] = useState<string | null>(null)
  const railRef = useRef<HTMLElement | null>(null)
  const turns = useMemo(() => messages.reduce<Array<{ user: ConversationMessage; assistant?: ConversationMessage; artifacts: ArtifactRecord[]; url?: string }>>((items, message) => {
    if (message.role === 'user') {
      items.push({ user: message, artifacts: [] })
      return items
    }
    if (message.role === 'assistant' && items.length) {
      const current = items.at(-1)!
      if (!current.assistant) current.assistant = message
    }
    return items
  }, []).map((turn) => {
    const content = `${turn.user.content}\n${turn.assistant?.content ?? ''}`
    return {
      ...turn,
      artifacts: artifacts.filter((artifact) => Boolean(turn.assistant?.runId) && artifact.runId === turn.assistant?.runId).slice(0, 2),
      url: content.match(/https?:\/\/[^\s)]+/)?.[0],
    }
  }), [artifacts, messages])

  useEffect(() => {
    if (!turns.length) {
      setActiveId(null)
      return
    }
    setActiveId((current) => current && turns.some((turn) => turn.user.id === current) ? current : turns.at(-1)!.user.id)
    const root = railRef.current?.closest('.fox-conversation') ?? null
    const anchors = turns.flatMap((turn) => {
      const anchor = document.getElementById(`fox-turn-${turn.user.id}`)
      return anchor ? [anchor] : []
    })
    if (!root || !anchors.length) return
    const visible = new Map<string, number>()
    const observer = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        const id = (entry.target as HTMLElement).dataset.turnMessageId
        if (!id) return
        if (entry.isIntersecting) visible.set(id, entry.intersectionRatio)
        else visible.delete(id)
      })
      const current = [...visible.entries()].sort((left, right) => right[1] - left[1])[0]?.[0]
      if (current) setActiveId(current)
    }, { root, threshold: [0.05, 0.25, 0.5, 0.8] })
    anchors.forEach((anchor) => observer.observe(anchor))
    return () => observer.disconnect()
  }, [turns])

  if (!turns.length) return null
  const hovered = turns.find((turn) => turn.user.id === hoveredId)
  const scrollToTurn = (messageId: string) => {
    setActiveId(messageId)
    document.getElementById(`fox-turn-${messageId}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  return <nav ref={railRef} className="fox-conversation-rail" aria-label="当前对话历史导航" onMouseLeave={() => setHoveredId(null)}>
    <div className="fox-conversation-marks">{turns.map((turn, index) => <button type="button" key={turn.user.id} className={turn.user.id === activeId ? 'is-active' : ''} aria-label={`第 ${index + 1} 轮：${turn.user.content.slice(0, 40)}`} onMouseEnter={() => setHoveredId(turn.user.id)} onFocus={() => setHoveredId(turn.user.id)} onBlur={() => setHoveredId(null)} onClick={() => scrollToTurn(turn.user.id)}><span /></button>)}</div>
    <div className={`fox-conversation-preview ${hovered ? 'is-visible' : ''}`}>
      <header><span><MessageCircleMore /></span><div><strong>{hovered?.user.content.slice(0, 58) || '对话回合'}</strong><small>{hovered ? `第 ${turns.indexOf(hovered) + 1} 轮 · ${new Date(hovered.user.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : ''}</small></div></header>
      <div className="fox-conversation-preview-answer"><small>Fox 回答</small><p>{splitAssistantContent(hovered?.assistant?.content ?? '').answer.slice(0, 180) || '正在等待 Fox 回答…'}</p></div>
      <div className="fox-conversation-resources">
        {hovered?.artifacts.map((artifact) => <span key={artifact.id}>{artifact.mediaType === 'text/html' || /html|web/i.test(artifact.artifactType) ? <Globe2 /> : artifact.mediaType?.startsWith('image/') ? <ImagePlus /> : <FileText />}<b>{artifact.displayName}</b></span>)}
        {hovered?.url && <span><Globe2 /><b>{hovered.url.replace(/^https?:\/\//, '').split('/')[0]}</b></span>}
        {hovered && hovered.artifacts.length === 0 && !hovered.url && <span className="is-fallback"><Sparkles /><b>Fox，你的智能助手</b></span>}
      </div>
    </div>
  </nav>
}

function ComposerAttachmentPreview() {
  const attachments = usePromptInputAttachments()
  if (!attachments.files.length) return null
  return (
    <Attachments variant="inline" className="fox-composer-attachments">
      {attachments.files.map((file) => (
        <Attachment key={file.id} data={file} onRemove={() => attachments.remove(file.id)}>
          <AttachmentPreview />
          <AttachmentInfo />
          <AttachmentRemove label="移除附件" />
        </Attachment>
      ))}
    </Attachments>
  )
}

const slashCommands = [
  { id: 'files', title: '读取文件', detail: '从当前工作区选择文件并加入上下文', icon: <FileText size={15} /> },
  { id: 'knowledge', title: '搜索知识库', detail: '使用知识检索服务查找相关资料', icon: <Library size={15} /> },
  { id: 'plan', title: '规划任务', detail: '先生成执行计划，再开始处理', icon: <ListTodo size={15} /> },
  { id: 'summarize', title: '总结对话', detail: '压缩当前会话并保留关键上下文', icon: <Sparkles size={15} /> }
]

const ComposerRuntimeContext = createContext<{
  knowledgeBindings: KnowledgeBindingRecord[]
  onKnowledge?: () => void
  onOpenSource?: (source: RuntimeSource) => void
  assistantName?: string
  runModel?: string
  questionRequest?: RuntimeQuestionRequest | null
  onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>
}>({ knowledgeBindings: [] })

const goalStateCopy: Record<ChatState, { label: string; detail: string; time: string }> = {
  complete: { label: '已完成', detail: '确认 Kun 授权范围与 Fox 开发边界', time: '刚刚' },
  running: { label: '进行中', detail: '正在整理当前问题的依据与结论', time: '12 分钟' },
  question: { label: '等待回答', detail: '需要你选择文件处理方式后继续', time: '待你确认' },
  approval: { label: '等待批准', detail: '需要授权后才能写入本地工作区', time: '待你批准' },
  error: { label: '连接错误', detail: '知识库服务暂时无响应，可重试当前步骤', time: '需处理' },
  denied: { label: '已取消', detail: '已取消本次写入，工作区没有变更', time: '刚刚' }
}

function GoalFloater({ chatState }: { chatState: ChatState }) {
  const [paused, setPaused] = useState(false)
  const goalState = paused ? { label: '已暂停', detail: '目标已暂停，继续后会回到当前任务', time: '暂停中' } : goalStateCopy[chatState]

  return (
    <div className={`fox-goal-floater is-${paused ? 'paused' : chatState}`}>
      <span className="fox-goal-icon"><Target size={15} /></span>
      <span className="fox-goal-copy">
        <span><strong>当前目标</strong><Badge>{goalState.label}</Badge></span>
        <small>{goalState.detail}</small>
      </span>
      <span className="fox-goal-time">{goalState.time}</span>
      <span className="fox-goal-actions">
        <IconButton label="编辑目标"><FileEdit size={13} /></IconButton>
        <IconButton label={paused ? '继续目标' : '暂停目标'} onClick={() => setPaused(!paused)}>{paused ? <Play size={13} /> : <Pause size={13} />}</IconButton>
        <IconButton label="清除目标"><Trash2 size={13} /></IconButton>
      </span>
    </div>
  )
}

function Composer({ resetKey, suggestedPrompt, chatState, showGoal = false, centered = false, runtimeControlled = false, projectRoot, projectPermissionMode, activeAgent, agents = [], modelService, runtimeCapabilities, yuxiModels = [], usage, knowledgeBindings = [], questionRequest, mascotSrc, mascotActive = false, onProject, onKnowledge, onAgentChange, onHeightChange, onPromptCommit, onSubmitPrompt, onSubmitQuestion, onStatusChange, onCancel }: { resetKey: number; suggestedPrompt?: string; chatState: ChatState; showGoal?: boolean; centered?: boolean; runtimeControlled?: boolean; projectRoot?: string | null; projectPermissionMode?: ProjectRecord['permissionMode'] | null; activeAgent?: AgentRecord | null; agents?: AgentRecord[]; modelService?: ModelServiceRecord | null; runtimeCapabilities?: Record<string, unknown>; yuxiModels?: YuxiModelRecord[]; usage?: ConversationUsage; knowledgeBindings?: KnowledgeBindingRecord[]; questionRequest?: RuntimeQuestionRequest | null; mascotSrc?: string; mascotActive?: boolean; onProject?: () => void; onKnowledge?: () => void; onAgentChange?: (agentId: string) => void; onHeightChange?: (height: number) => void; onPromptCommit?: (prompt: string) => void; onSubmitPrompt?: (prompt: string, model?: string, files?: Array<{ filename?: string; mediaType?: string; url?: string }>) => 'complete' | 'question' | 'approval' | 'error' | void | Promise<'complete' | 'question' | 'approval' | 'error' | void>; onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>; onStatusChange?: (status: 'ready' | 'streaming') => void; onCancel?: () => void | Promise<void> }) {
  const runtimeContext = useContext(ComposerRuntimeContext)
  const activeQuestionRequest = questionRequest ?? runtimeContext.questionRequest
  const submitQuestion = onSubmitQuestion ?? runtimeContext.onSubmitQuestion
  const activeKnowledgeBindings = knowledgeBindings.length ? knowledgeBindings : runtimeContext.knowledgeBindings
  const enabledKnowledgeBindings = activeKnowledgeBindings.filter((binding) => binding.enabled !== false)
  const openKnowledge = onKnowledge ?? runtimeContext.onKnowledge
  const [status, setStatus] = useState<'ready' | 'streaming'>('ready')
  const [focused, setFocused] = useState(false)
  const sendWithModifier = typeof window !== 'undefined' && window.localStorage.getItem('fox.preferences.sendKey') === 'mod-enter'
  const spellcheckEnabled = typeof window === 'undefined' || window.localStorage.getItem('fox.preferences.spellcheck') !== '0'
  const voiceEnabled = typeof window === 'undefined' || window.localStorage.getItem('fox.preferences.voice') !== '0'
  const allowYuxiModelOverride = yuxiModelOverrideAllowed(activeAgent)
  const isYuxiAgent = activeAgent?.runtimeType === 'yuxi'
  const supportsImageInput = isYuxiAgent || runtimeCapabilities?.imageInput === true
  const supportsDynamicModelSwitch = isYuxiAgent
    ? allowYuxiModelOverride
    : runtimeCapabilities?.dynamicModelSwitch === true
  const modelOptions = activeAgent?.runtimeType === 'yuxi' && allowYuxiModelOverride
    ? yuxiModels
    : modelService?.modelId
      ? [{ spec: modelService.modelId, modelId: modelService.modelId, displayName: modelService.modelId, providerId: 'fox', providerDisplayName: modelService.name }]
      : []
  const defaultModel = activeAgent?.runtimeType === 'yuxi'
    ? activeAgent.defaultModel
    : modelService?.modelId ?? activeAgent?.defaultModel ?? '未配置模型'
  const [model, setModel] = useState(defaultModel)
  const [draft, setDraft] = useState('')
  const [questionSelections, setQuestionSelections] = useState<Record<string, string[]>>({})
  const [questionOtherAnswers, setQuestionOtherAnswers] = useState<Record<string, string>>({})
  const [commandOpen, setCommandOpen] = useState(false)
  const [mentionOpen, setMentionOpen] = useState(false)
  const submitTimer = useRef<number | null>(null)
  const questionSubmittingRef = useRef(false)
  const composerRef = useRef<HTMLDivElement | null>(null)
  const promptRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => () => {
    if (submitTimer.current !== null) window.clearTimeout(submitTimer.current)
  }, [])

  useEffect(() => {
    if (submitTimer.current !== null) {
      window.clearTimeout(submitTimer.current)
      submitTimer.current = null
    }
    setStatus('ready')
    setDraft(suggestedPrompt ?? '')
    setCommandOpen(false)
    setMentionOpen(false)
  }, [resetKey, suggestedPrompt])

  useEffect(() => {
    setModel(defaultModel)
  }, [defaultModel])

  useEffect(() => {
    setQuestionSelections({})
    setQuestionOtherAnswers({})
    questionSubmittingRef.current = false
    if (activeQuestionRequest) setStatus('ready')
  }, [activeQuestionRequest?.runId])

  useEffect(() => {
    if (!runtimeControlled) return
    setStatus(chatState === 'running' ? 'streaming' : 'ready')
  }, [chatState, runtimeControlled])

  useEffect(() => {
    const composer = composerRef.current
    const prompt = promptRef.current
    const stage = composer?.closest<HTMLElement>('.fox-chat-stage')
    if (!composer || !prompt || !stage || !onHeightChange) return

    let animationFrame = 0
    const updateHeight = () => {
      window.cancelAnimationFrame(animationFrame)
      animationFrame = window.requestAnimationFrame(() => {
        const stageBottom = stage.getBoundingClientRect().bottom
        const promptTop = prompt.getBoundingClientRect().top
        onHeightChange(Math.ceil(Math.max(0, stageBottom - promptTop + 6)))
      })
    }

    updateHeight()
    const observer = new ResizeObserver(updateHeight)
    observer.observe(stage)
    observer.observe(composer)
    observer.observe(prompt)
    window.addEventListener('resize', updateHeight)

    return () => {
      window.cancelAnimationFrame(animationFrame)
      observer.disconnect()
      window.removeEventListener('resize', updateHeight)
    }
  }, [onHeightChange])

  const onDraftChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    const value = event.currentTarget.value
    setDraft(value)
    setCommandOpen(value.startsWith('/'))
    setMentionOpen(!value.startsWith('/') && /(^|\s)@[\w./-]*$/.test(value))
  }

  const applySlashCommand = (command: typeof slashCommands[number]) => {
    const next = command.id === 'files' ? '请读取并分析 @docs/FOX_ARCHITECTURE.md' : command.id === 'knowledge' ? '请从知识库搜索 Fox 架构相关资料' : command.id === 'plan' ? '请先规划接下来的开发步骤' : '请总结当前对话'
    setDraft(next)
    setCommandOpen(false)
  }

  const applyMention = (path: string) => {
    setDraft((value) => value.replace(/@[\w./-]*$/, `@${path} `))
    setMentionOpen(false)
  }

  const selectQuestionOption = (question: RuntimeQuestion, value: string) => {
    setQuestionSelections((current) => {
      const selected = current[question.questionId] ?? []
      return {
        ...current,
        [question.questionId]: question.multiSelect
          ? selected.includes(value) ? selected.filter((item) => item !== value) : [...selected, value]
          : [value],
      }
    })
  }

  const questionSubmitting = Boolean(activeQuestionRequest && status === 'streaming')
  const buildQuestionAnswer = (otherText: string) => {
    if (!activeQuestionRequest) return null
    const answers: Record<string, string | string[]> = {}
    const lines: string[] = []
    for (const question of activeQuestionRequest.questions) {
      const selected = questionSelections[question.questionId] ?? []
      if (selected.length) {
        answers[question.questionId] = question.multiSelect ? selected : selected[0]
        const labels = selected.map((value) => question.options.find((option) => option.value === value)?.label ?? value)
        lines.push(`${question.question}：${labels.join('、')}`)
      } else {
        const individualAnswer = questionOtherAnswers[question.questionId]?.trim()
        const fallbackAnswer = activeQuestionRequest.questions.length === 1 ? otherText.trim() : ''
        const openAnswer = individualAnswer || fallbackAnswer
        if (!question.allowOther || !openAnswer) return null
        answers[question.questionId] = openAnswer
        lines.push(`${question.question}：${openAnswer}`)
      }
    }
    if (otherText.trim() && !Object.values(answers).some((value) => value === otherText.trim())) {
      lines.push(`其他补充：${otherText.trim()}`)
    }
    return { answers, text: lines.join('\n') }
  }

  return (
    <div ref={composerRef} className={`fox-composer-wrap ${centered ? 'is-empty' : ''}`}>
      {showGoal && <GoalFloater chatState={chatState} />}
      {mascotSrc && <div className={`fox-composer-mascot ${mascotActive ? 'is-active' : ''}`} aria-hidden="true"><img key={mascotSrc} src={mascotSrc} alt="" /></div>}
      <Collapsible open={Boolean(activeQuestionRequest)}>
        <CollapsibleContent className="fox-question-drawer">
          {activeQuestionRequest && <div className="fox-question-drawer-content">
            <header><span><CircleHelp size={15} /></span><div><strong>需要你补充一点信息</strong><small>回答后 Fox 会继续刚才的任务</small></div></header>
            <div className="fox-question-drawer-list">
              {activeQuestionRequest.questions.map((question, index) => isConfirmationQuestion(question) ? (
                <Confirmation key={question.questionId} approval={{ id: question.questionId }} state="approval-requested" className="fox-question-confirmation">
                  <ConfirmationRequest>
                    <ConfirmationTitle>{question.question}</ConfirmationTitle>
                    <ConfirmationActions>
                      {question.options.map((option) => <ConfirmationAction key={option.value} disabled={questionSubmitting} variant={(questionSelections[question.questionId] ?? []).includes(option.value) ? 'default' : 'outline'} onClick={() => selectQuestionOption(question, option.value)}>{option.label}</ConfirmationAction>)}
                    </ConfirmationActions>
                  </ConfirmationRequest>
                </Confirmation>
              ) : (
                <section key={question.questionId} className="fox-question-field">
                  <div><b>{activeQuestionRequest.questions.length > 1 ? `${index + 1}. ` : ''}{question.question}</b>{question.multiSelect && <small>可多选</small>}</div>
                  <Suggestions className="fox-question-suggestions">
                    {question.options.map((option) => <Suggestion key={option.value} suggestion={option.value} disabled={questionSubmitting} variant={(questionSelections[question.questionId] ?? []).includes(option.value) ? 'default' : 'outline'} onClick={() => selectQuestionOption(question, option.value)}>{option.label}</Suggestion>)}
                  </Suggestions>
                  {question.allowOther && activeQuestionRequest.questions.length > 1 && <Input disabled={questionSubmitting} value={questionOtherAnswers[question.questionId] ?? ''} onChange={(event) => setQuestionOtherAnswers((current) => ({ ...current, [question.questionId]: event.target.value }))} placeholder="输入这一项的补充答案" />}
                </section>
              ))}
            </div>
            {activeQuestionRequest.questions.some((question) => question.allowOther) && <p>也可以直接在下方输入框补充自己的答案。</p>}
          </div>}
        </CollapsibleContent>
      </Collapsible>
      <div ref={promptRef} className="fox-prompt-shell" onFocusCapture={() => setFocused(true)} onBlurCapture={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false) }}>
        <BorderBeam
          active={centered && focused}
          borderRadius={24}
          brightness={1.06}
          className="fox-prompt-border-beam"
          colorVariant="mono"
          duration={2.5}
          size="pulse-inner"
          strength={1}
          style={{
            '--beam-stroke-opacity': 1.65,
            '--beam-inner-opacity': 1.45,
            '--beam-bloom-opacity': 1.7
          } as React.CSSProperties}
          theme="light"
        >
        <PromptInput
          accept={supportsImageInput ? 'image/*,.pdf,.txt,.md,.docx' : '.pdf,.txt,.md,.docx'}
          multiple
          maxFiles={8}
          maxFileSize={5 * 1024 * 1024}
          onError={({ code }) => {
            if (code === 'max_file_size') toast.error('单个附件不能超过 5 MB')
            else if (code === 'max_files') toast.error('一次最多添加 8 个附件')
            else toast.error('不支持这个附件格式')
          }}
          onSubmitStart={({ text, files }) => {
            const prompt = text.trim() || (files.length ? '请分析这些附件' : '')
            if (!prompt && !activeQuestionRequest) return
            if (!activeQuestionRequest) flushSync(() => onPromptCommit?.(prompt))
          }}
          onSubmit={async ({ text, files: submittedFiles }) => {
          if (activeQuestionRequest) {
            if (questionSubmittingRef.current) return
            const answer = buildQuestionAnswer(text)
            if (!answer) {
              toast.error('请先完成需要补充的事项')
              return
            }
            questionSubmittingRef.current = true
            setStatus('streaming')
            const submitted = await submitQuestion?.(answer.text, answer.answers)
            if (!submitted) {
              questionSubmittingRef.current = false
              setStatus('ready')
              return
            }
            setDraft('')
            setQuestionSelections({})
            setQuestionOtherAnswers({})
            onStatusChange?.('streaming')
            return
          }
          if (!text.trim() && submittedFiles.length === 0) return
          setStatus('streaming')
          setDraft('')
          setCommandOpen(false)
          setMentionOpen(false)
          const selectedModel = activeAgent?.runtimeType === 'yuxi' && !allowYuxiModelOverride
            ? undefined
            : model === '未配置模型'
              ? undefined
              : model
          const outcome = await onSubmitPrompt?.(text.trim() || '请分析这些附件', selectedModel, submittedFiles)
          if (outcome === 'question' || outcome === 'approval' || outcome === 'error') {
            setStatus('ready')
            onStatusChange?.('ready')
            return
          }
          onStatusChange?.('streaming')
          if (runtimeControlled) return
          if (submitTimer.current !== null) window.clearTimeout(submitTimer.current)
          submitTimer.current = window.setTimeout(() => {
            setStatus('ready')
            onStatusChange?.('ready')
            submitTimer.current = null
          }, 3600)
        }}
          className={`fox-prompt-input ${focused ? 'is-focused' : ''}`}
        >
        {commandOpen && <Command className="fox-slash-menu">
          <div className="fox-command-title"><span>命令</span><kbd>Esc</kbd></div>
          <CommandList>
            <CommandEmpty>没有匹配的命令</CommandEmpty>
            <CommandGroup>
              {slashCommands.map((command) => <CommandItem key={command.id} value={command.id} onSelect={() => applySlashCommand(command)}><span className="fox-command-icon">{command.icon}</span><span><strong>/{command.id}</strong><small>{command.title} · {command.detail}</small></span><kbd>↵</kbd></CommandItem>)}
            </CommandGroup>
          </CommandList>
        </Command>}
        {mentionOpen && <div className="fox-mention-menu"><div className="fox-command-title"><span>引用工作区文件</span><kbd>@</kbd></div>{['docs/FOX_ARCHITECTURE.md', 'apps/desktop/src/features/chat/workbench.tsx', 'apps/desktop/src/styles/workbench.css'].map((path) => <button type="button" key={path} onMouseDown={(event) => event.preventDefault()} onClick={() => applyMention(path)}><FileText size={14} /><span>{path}</span></button>)}</div>}
        <ComposerAttachmentPreview />
        {runtimeControlled && (projectRoot || enabledKnowledgeBindings.length > 0) && (
          <div className="fox-composer-context">
            {projectRoot && (
              <button type="button" className="is-authorized" onClick={onProject} title={projectRoot}>
                <FolderOpen size={13} />
                <span>{projectRoot.split(/[\\/]/).filter(Boolean).at(-1)}</span>
                <Check size={12} />
              </button>
            )}
            {enabledKnowledgeBindings.map((binding) => {
              const label = binding.knowledgeBaseName || binding.knowledgeBaseId
              return (
                <button
                  type="button"
                  className="is-authorized is-knowledge"
                  key={`${binding.serviceConnectionId}:${binding.knowledgeBaseId}`}
                  onClick={openKnowledge}
                  title={label}
                >
                  <Library size={13} />
                  <span>{label}</span>
                  <Check size={12} />
                </button>
              )
            })}
          </div>
        )}
        <PromptInputBody>
          <PromptInputTextarea value={draft} disabled={questionSubmitting} spellCheck={spellcheckEnabled} onChange={onDraftChange} onKeyDown={(event: KeyboardEvent<HTMLTextAreaElement>) => { if (event.key === 'Escape') { setCommandOpen(false); setMentionOpen(false) } if (event.key === 'Enter' && sendWithModifier) { if (event.ctrlKey || event.metaKey) return; event.preventDefault(); const textarea = event.currentTarget; textarea.setRangeText('\n', textarea.selectionStart, textarea.selectionEnd, 'end'); setDraft(textarea.value) } }} className="fox-prompt-textarea" placeholder={activeQuestionRequest ? '补充你的答案，或直接选择上方选项' : '给 Fox 发消息，输入 / 查看命令，@ 引用文件…'} />
        </PromptInputBody>
        <PromptInputFooter className="fox-prompt-toolbar">
          <PromptInputTools className="fox-composer-tools">
            <PromptInputActionMenu>
              <PromptInputActionMenuTrigger tooltip="添加内容"><Plus size={18} /></PromptInputActionMenuTrigger>
              <PromptInputActionMenuContent className="fox-add-menu">
                <PromptInputActionAddAttachments label="添加文件" />
                {supportsImageInput && <PromptInputActionAddAttachments label="添加图片" />}
                <PromptInputActionMenuItem onSelect={openKnowledge}><Library />{enabledKnowledgeBindings.length ? `知识库（已选 ${enabledKnowledgeBindings.length}）` : '选择知识库'}</PromptInputActionMenuItem>
              </PromptInputActionMenuContent>
            </PromptInputActionMenu>
            <PromptInputSelect value={projectPermissionMode ?? 'read_only'} onValueChange={() => onProject?.()} disabled={!runtimeControlled}>
              <PromptInputSelectTrigger className="fox-execution-control"><Terminal size={14} /><PromptInputSelectValue /></PromptInputSelectTrigger>
              <PromptInputSelectContent>
                <PromptInputSelectItem value="ask">执行前询问</PromptInputSelectItem>
                <PromptInputSelectItem value="allow">自动执行</PromptInputSelectItem>
                <PromptInputSelectItem value="read_only">只读模式</PromptInputSelectItem>
              </PromptInputSelectContent>
            </PromptInputSelect>
          </PromptInputTools>
          <div className="fox-composer-submit">
            <AIContext usedTokens={usage?.totalTokens ?? 0} maxTokens={modelService?.contextWindow ?? 1}>
              <AIContextTrigger>
                <button type="button" className="fox-context-capacity" aria-label={`上下文已使用 ${Math.min(100, Math.round(((usage?.totalTokens ?? 0) / Math.max(1, modelService?.contextWindow ?? 1)) * 100))}%`} style={{ '--fox-context-used': `${Math.min(100, Math.round(((usage?.totalTokens ?? 0) / Math.max(1, modelService?.contextWindow ?? 1)) * 100))}%` } as React.CSSProperties}><span /></button>
              </AIContextTrigger>
              <AIContextContent side="top" align="end" sideOffset={10} className="fox-context-popover">
                <AIContextContentHeader />
                <AIContextContentBody className="fox-context-breakdown">
                  <div><span>输入</span><strong>{compactTokenCount(usage?.inputTokens ?? 0)} tokens</strong></div>
                  <div><span>输出</span><strong>{compactTokenCount(usage?.outputTokens ?? 0)} tokens</strong></div>
                  <div><span>可用</span><strong>{compactTokenCount(Math.max(0, (modelService?.contextWindow ?? 0) - (usage?.totalTokens ?? 0)))} tokens</strong></div>
                </AIContextContentBody>
                <AIContextContentFooter className="fox-context-popover-footer">
                  <span>{model}</span><span>{Math.min(100, Math.round(((usage?.totalTokens ?? 0) / Math.max(1, modelService?.contextWindow ?? 1)) * 100))}%</span>
                </AIContextContentFooter>
              </AIContextContent>
            </AIContext>
            {supportsDynamicModelSwitch ? <PromptInputSelect value={model} onValueChange={setModel} disabled={modelOptions.length === 0}>
            <PromptInputSelectTrigger className="fox-model-control"><PromptInputSelectValue /></PromptInputSelectTrigger>
              <PromptInputSelectContent>{modelOptions.length ? modelOptions.map((item) => <PromptInputSelectItem key={item.spec} value={item.spec}>{item.displayName || item.modelId}</PromptInputSelectItem>) : <PromptInputSelectItem value={model}>{model}</PromptInputSelectItem>}</PromptInputSelectContent>
            </PromptInputSelect> : <span className="fox-model-control fox-model-static" title="当前 Runtime 不支持会话中动态切换模型">{model}</span>}
            <PromptInputSelect value={activeAgent?.id ?? ''} onValueChange={(value) => value !== activeAgent?.id && onAgentChange?.(value)} disabled={!activeAgent}>
              <PromptInputSelectTrigger className="fox-agent-control"><Sparkles size={14} /><PromptInputSelectValue /></PromptInputSelectTrigger>
              <PromptInputSelectContent>
                {agents.map((item) => <PromptInputSelectItem key={item.id} value={item.id} disabled={!item.available}>{item.name}</PromptInputSelectItem>)}
              </PromptInputSelectContent>
            </PromptInputSelect>
            {voiceEnabled && <PromptInputButton className="fox-voice-button" tooltip="语音输入"><Mic size={17} /></PromptInputButton>}
            <PromptInputSubmit
              status={status}
              onStop={() => {
                if (submitTimer.current !== null) {
                  window.clearTimeout(submitTimer.current)
                  submitTimer.current = null
                }
                setStatus('ready')
                onStatusChange?.('ready')
                void onCancel?.()
              }}
              className="fox-send-button"
            >
              {status === 'streaming' ? <CircleStop size={17} /> : <ArrowUp size={18} />}
            </PromptInputSubmit>
          </div>
        </PromptInputFooter>
        </PromptInput>
        </BorderBeam>
      </div>
      <div className="fox-composer-meta"><span><i />{projectRoot ? `已授权 · ${projectRoot}` : '未授权项目目录'} · 知识库可选</span><span>{compactTokenCount(usage?.totalTokens ?? 0)} tokens · 上下文 {Math.min(100, Math.round(((usage?.totalTokens ?? 0) / Math.max(1, modelService?.contextWindow ?? 1)) * 100))}%</span><span>Fox 可能会出错，请核对重要信息。</span></div>
    </div>
  )
}

function ProjectAccessDialog({ open, currentRoot, projects, busy, browsing, error, onOpenChange, onBrowse, onConfirm }: { open: boolean; currentRoot?: string | null; projects: ProjectRecord[]; busy: boolean; browsing: boolean; error?: string | null; onOpenChange: (open: boolean) => void; onBrowse: () => Promise<string | null>; onConfirm: (path: string, permissionMode: ProjectRecord['permissionMode']) => void }) {
  const [path, setPath] = useState(currentRoot ?? '')
  const currentProject = projects.find((project) => project.rootPath === currentRoot)
  const storedDefaultPermission = typeof window === 'undefined'
    ? 'ask'
    : window.localStorage.getItem('fox.preferences.defaultPermission')
  const defaultPermission = storedDefaultPermission === 'read_only' || storedDefaultPermission === 'allow'
    ? storedDefaultPermission
    : 'ask'
  const [permissionMode, setPermissionMode] = useState<ProjectRecord['permissionMode']>(currentProject?.permissionMode ?? defaultPermission)

  useEffect(() => {
    if (!open) return
    const project = projects.find((item) => item.rootPath === currentRoot)
    setPath(currentRoot ?? '')
    setPermissionMode(project?.permissionMode ?? defaultPermission)
  }, [currentRoot, defaultPermission, open, projects])

  const selectProject = (project: ProjectRecord) => {
    setPath(project.rootPath)
    setPermissionMode(project.permissionMode)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="fox-project-dialog">
        <DialogHeader>
          <DialogTitle>授权项目文件夹</DialogTitle>
          <DialogDescription>Fox 会把此目录加入当前对话草稿；发送第一条消息后才会创建并保存会话。所有文件工具都受项目边界和所选权限模式约束。</DialogDescription>
        </DialogHeader>
        {projects.length > 0 && <div className="fox-project-dialog-existing"><span>已有项目</span><div>{projects.map((project) => <button type="button" key={project.id} className={path === project.rootPath ? 'is-selected' : ''} onClick={() => selectProject(project)}><FolderOpen size={14} /><span><strong>{project.name}</strong><small>{project.rootPath}</small></span><Badge variant="outline">{{ read_only: '只读', ask: '询问', allow: '允许' }[project.permissionMode]}</Badge></button>)}</div></div>}
        <label><span>本机文件夹路径</span><div className="fox-project-path-control"><Input autoFocus value={path} onChange={(event) => setPath(event.target.value)} placeholder="D:\\projects\\my-project" onKeyDown={(event) => { if (event.key === 'Enter' && path.trim()) onConfirm(path.trim(), permissionMode) }} /><Button type="button" variant="outline" disabled={browsing} onClick={() => void onBrowse().then((selected) => selected && setPath(selected))}>{browsing ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}浏览</Button></div></label>
        <div className="fox-project-dialog-permissions"><span>执行方式</span><div>{([
          ['read_only', '只读', '只允许读取、搜索和分析'],
          ['ask', '询问', '写入或执行前请求批准'],
          ['allow', '允许', '在授权范围内自动执行']
        ] as const).map(([value, label, description]) => <button type="button" key={value} className={permissionMode === value ? 'is-selected' : ''} onClick={() => setPermissionMode(value)}><strong>{label}</strong><small>{description}</small></button>)}</div><p>当前版本尚未注册写入和命令工具；“询问”和“允许”会先保存策略，待安全执行链路启用后生效。</p></div>
        {error && <p className="fox-project-dialog-error">{error}</p>}
        <DialogFooter><Button variant="outline" onClick={() => onOpenChange(false)}>取消</Button><Button disabled={busy || !path.trim()} onClick={() => onConfirm(path.trim(), permissionMode)}>{busy && <LoaderCircle className="animate-spin" />}使用此项目</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function KnowledgeBindingDialog({ open, items, bindings, busy, error, onOpenChange, onConfirm }: { open: boolean; items: KnowledgeBaseRecord[]; bindings: KnowledgeBindingRecord[]; busy: boolean; error?: string | null; onOpenChange: (open: boolean) => void; onConfirm: (items: KnowledgeBaseRecord[]) => void }) {
  const [selected, setSelected] = useState<string[]>([])

  useEffect(() => {
    if (open) setSelected(bindings.filter((item) => item.enabled).map((item) => item.knowledgeBaseId))
  }, [bindings, open])

  const toggle = (id: string) => setSelected((current) => current.includes(id)
    ? current.filter((item) => item !== id)
    : [...current, id])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="fox-knowledge-binding-dialog">
        <DialogHeader>
          <DialogTitle>选择会话知识库</DialogTitle>
          <DialogDescription>只会把你明确选择且有权访问的知识库开放给当前会话。Agent 与知识库仍保持独立。</DialogDescription>
        </DialogHeader>
        <Command className="fox-knowledge-binding-command">
          <CommandList>
            <CommandEmpty>当前没有可访问的知识库，请先连接并登录知识库服务。</CommandEmpty>
            <CommandGroup>
              {items.map((item) => {
                const checked = selected.includes(item.id)
                return <CommandItem key={item.id} value={`${item.name} ${item.description}`} onSelect={() => toggle(item.id)}><span className={`fox-knowledge-check ${checked ? 'is-checked' : ''}`}>{checked && <Check />}</span><span><strong>{item.name}</strong><small>{item.description || `${item.fileCount} 个文件`}</small></span></CommandItem>
              })}
            </CommandGroup>
          </CommandList>
        </Command>
        {error && <p className="fox-setting-error">{error}</p>}
        <DialogFooter><Button variant="outline" onClick={() => onOpenChange(false)}>取消</Button><Button disabled={busy} onClick={() => onConfirm(items.filter((item) => selected.includes(item.id)))}>{busy && <LoaderCircle className="animate-spin" />}保存选择</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function ConversationManagementDialogs({ conversation, mode, busy, onClose, onRename, onDelete }: { conversation: { id: string; title: string } | null; mode: 'rename' | 'delete' | null; busy: boolean; onClose: () => void; onRename: (title: string) => void; onDelete: () => void }) {
  const [title, setTitle] = useState('')
  useEffect(() => { if (conversation && mode === 'rename') setTitle(conversation.title) }, [conversation, mode])
  return <>
    <Dialog open={mode === 'rename'} onOpenChange={(open) => { if (!open) onClose() }}>
      <DialogContent className="fox-conversation-dialog">
        <DialogHeader><DialogTitle>重命名对话</DialogTitle><DialogDescription>新的标题只改变显示名称，不会改变当前会话绑定的 Agent 或 Runtime。</DialogDescription></DialogHeader>
        <Input autoFocus value={title} maxLength={120} onChange={(event) => setTitle(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && title.trim() && !busy) onRename(title.trim()) }} />
        <DialogFooter><Button variant="outline" onClick={onClose}>取消</Button><Button disabled={busy || !title.trim()} onClick={() => onRename(title.trim())}>{busy && <LoaderCircle className="animate-spin" />}保存</Button></DialogFooter>
      </DialogContent>
    </Dialog>
    <Dialog open={mode === 'delete'} onOpenChange={(open) => { if (!open) onClose() }}>
      <DialogContent className="fox-conversation-dialog">
        <DialogHeader><DialogTitle>删除对话？</DialogTitle><DialogDescription>“{conversation?.title}”及其消息、工具与审批记录、Fox 内部附件和未导出产物、Runtime Session，以及对应的远程线程（如有）将被删除。已导出到项目目录的文件不会受影响。此操作无法撤销。</DialogDescription></DialogHeader>
        <DialogFooter><Button variant="outline" onClick={onClose}>取消</Button><Button variant="destructive" disabled={busy} onClick={onDelete}>{busy && <LoaderCircle className="animate-spin" />}删除</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}

function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

function toolTarget(tool: ToolCallRecord) {
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
  return compactValue(input.path ?? input.filePath ?? input.target, tool.toolName)
}

function toolDiff(tool: ToolCallRecord, approvals: ApprovalRecord[]) {
  const approval = approvals.find((item) => item.toolCallId === tool.id)
  if (approval?.request.diff) return approval.request.diff
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
  if (typeof input.content === 'string') return input.content.split('\n').map((line) => `+ ${line}`).join('\n')
  if (typeof input.oldText === 'string' || typeof input.newText === 'string') {
    return [
      ...String(input.oldText ?? '').split('\n').map((line) => `- ${line}`),
      ...String(input.newText ?? '').split('\n').map((line) => `+ ${line}`),
    ].join('\n')
  }
  return JSON.stringify(tool.input, null, 2)
}

function projectFileTree(entries: ProjectFileEntry[], workspaceName: string, selectedFile: string, onSelect: (path: string) => void) {
  const workspaceRootPath = '__fox_workspace_root__'
  const byParent = new Map<string, ProjectFileEntry[]>()
  entries.forEach((entry) => byParent.set(entry.parent, [...(byParent.get(entry.parent) ?? []), entry]))
  const render = (parent: string): ReactNode => (byParent.get(parent) ?? []).map((entry) => entry.isDirectory
    ? <FileTreeFolder key={entry.path} path={entry.path} name={entry.name}>{render(entry.path)}</FileTreeFolder>
    : <FileTreeFile key={entry.path} path={entry.path} name={entry.name} />)
  return <FileTree defaultExpanded={new Set([workspaceRootPath])} selectedPath={selectedFile} onSelect={(path) => { if (path !== workspaceRootPath && !entries.find((item) => item.path === path)?.isDirectory) onSelect(path) }} className="fox-ai-file-tree"><FileTreeFolder path={workspaceRootPath} name={workspaceName}>{render('')}</FileTreeFolder></FileTree>
}

function ContextContent({ mode, detail }: { mode: RightMode; detail: ConversationDetail | null }) {
  const conversationId = detail?.conversation.id ?? null
  const [selectedFile, setSelectedFile] = useState('')
  const [selectedChange, setSelectedChange] = useState('')
  const [fileSearch, setFileSearch] = useState('')
  const [projectFiles, setProjectFiles] = useState<ProjectFileEntry[]>([])
  const [fileLoading, setFileLoading] = useState(false)
  const [fileError, setFileError] = useState<string | null>(null)

  useEffect(() => {
    if (mode !== 'files' || !conversationId) return
    let cancelled = false
    setFileLoading(true)
    setFileError(null)
    void desktopClient.listProjectFiles(conversationId).then((items) => {
      if (cancelled) return
      setProjectFiles(items)
      const firstFile = items.find((item) => !item.isDirectory)?.path ?? ''
      setSelectedFile((current) => items.some((item) => item.path === current && !item.isDirectory) ? current : firstFile)
    }).catch((cause) => {
      if (!cancelled) setFileError(cause instanceof Error ? cause.message : String(cause))
    }).finally(() => { if (!cancelled) setFileLoading(false) })
    return () => { cancelled = true }
  }, [conversationId, mode])

  if (mode === 'files') {
    const normalizedSearch = fileSearch.trim().toLowerCase()
    const visibleFiles = projectFiles.filter((item) => !item.isDirectory && (!normalizedSearch || item.path.toLowerCase().includes(normalizedSearch)))
    if (!conversationId || !detail?.conversation.projectRoot) return <div className="fox-empty-panel"><Folders size={29} /><strong>尚未选择项目</strong><span>为当前会话授权一个项目文件夹后即可浏览。</span></div>
    return (
      <div className="fox-file-panel is-tree-only">
        <div className="fox-file-tree-pane">
          <div className="fox-panel-search"><Search size={14} /><input value={fileSearch} onChange={(event) => setFileSearch(event.target.value)} placeholder="搜索文件" />{fileSearch && <button type="button" aria-label="清除搜索" onClick={() => setFileSearch('')}><X size={12} /></button>}</div>
          <ScrollArea className="fox-file-tree">
            {fileLoading ? <div className="fox-panel-loading"><LoaderCircle className="animate-spin" />正在读取项目文件</div> : normalizedSearch ? <div className="fox-file-search-results">{visibleFiles.map((item) => <button type="button" key={item.path} className={selectedFile === item.path ? 'is-active' : ''} onClick={() => setSelectedFile(item.path)}><FileText size={14} /><span><strong>{item.name}</strong><small>{item.parent || detail.conversation.projectRoot}</small></span></button>)}{visibleFiles.length === 0 && <p>没有匹配的文件</p>}</div> : projectFileTree(projectFiles, detail.conversation.projectRoot.split(/[\\/]/).at(-1) || detail.conversation.projectRoot, selectedFile, setSelectedFile)}
          </ScrollArea>
          {fileError && <p className="fox-file-tree-error"><AlertTriangle size={14} />{fileError}</p>}
        </div>
      </div>
    )
  }
  if (mode === 'knowledge') {
    const sources = runtimeProcess(detail?.runtimeEvents ?? []).sources
    const items = [
      ...(detail?.knowledgeBindings ?? []).map((item) => ({ id: `binding:${item.knowledgeBaseId}`, title: item.knowledgeBaseName || item.knowledgeBaseId, detail: '已绑定到当前会话', kind: '知识库' })),
      ...sources.map((item) => ({ id: `source:${item.id}`, title: item.title, detail: item.excerpt || '本次回答引用的知识来源', kind: '引用' })),
    ].filter((item, index, all) => all.findIndex((candidate) => candidate.id === item.id) === index)
    if (!items.length) return <div className="fox-empty-panel"><BookOpen size={29} /><strong>当前会话没有知识来源</strong><span>从输入框的“+”绑定知识库，检索结果也会保留在这里。</span></div>
    return <ScrollArea className="fox-source-list">{items.map((item, index) => <button type="button" key={item.id}><span>{index + 1}</span><div><strong>{item.title}</strong><small>{item.kind} · {item.detail}</small></div></button>)}</ScrollArea>
  }
  if (mode === 'changes') {
    const changeFiles = (detail?.toolCalls ?? []).filter((item) => /write|edit|patch|create/i.test(item.toolName))
    const activeChange = changeFiles.find((item) => item.id === selectedChange) ?? changeFiles.at(-1)
    const diffLines = activeChange ? toolDiff(activeChange, detail?.approvals ?? []).split('\n').map((line) => [line.startsWith('+') ? 'added' : line.startsWith('-') ? 'removed' : line.startsWith('@@') ? 'meta' : 'context', line]) : []
    if (!changeFiles.length) return <div className="fox-empty-panel"><FileEdit size={29} /><strong>当前会话没有文件更改</strong><span>Fox 的写入和编辑工具记录会显示在这里。</span></div>
    return (
      <div className="fox-change-panel">
        <div className="fox-change-list">
          {changeFiles.map((item) => <button type="button" key={item.id} className={activeChange?.id === item.id ? 'is-active' : ''} onClick={() => setSelectedChange(item.id)}><FileEdit size={14} /><span><strong>{toolTarget(item)}</strong><small>{item.toolName} · {item.status === 'completed' ? '已完成' : item.status === 'failed' ? '失败' : '处理中'}</small></span></button>)}
        </div>
        <div className="fox-diff-view">
          <div className="fox-diff-head"><FileEdit size={14} /><strong>{activeChange ? toolTarget(activeChange) : '更改详情'}</strong><span><Badge variant="outline">{activeChange?.status ?? '未知'}</Badge></span></div>
          <ScrollArea className="fox-diff-scroll"><code>{diffLines.map(([tone, line], index) => <span key={index} className={`is-${tone}`}><i>{index + 1}</i><b>{line || ' '}</b></span>)}</code></ScrollArea>
        </div>
      </div>
    )
  }
  if (mode === 'todo') {
    const tasks = detail?.toolCalls ?? []
    const completed = tasks.filter((item) => item.status === 'completed').length
    const running = tasks.filter((item) => !['completed', 'failed', 'denied', 'cancelled'].includes(item.status)).length
    const pending = tasks.length - completed - running
    if (!tasks.length) return <div className="fox-empty-panel"><ListTodo size={29} /><strong>当前会话暂无执行步骤</strong><span>工具调用开始后，这里会像 Kun 一样持续更新任务状态。</span></div>
    return <div className="fox-todo-panel"><div className="fox-todo-stats"><span><strong>{pending}</strong><small>待处理</small></span><span><strong>{running}</strong><small>进行中</small></span><span><strong>{completed}</strong><small>已完成</small></span></div><Queue className="fox-ai-queue"><QueueList className="fox-ai-queue-list">{tasks.map((item) => { const done = item.status === 'completed'; const active = !done && !['failed', 'denied', 'cancelled'].includes(item.status); return <QueueItem key={item.id} className="fox-ai-queue-item"><div className="fox-ai-queue-row"><QueueItemIndicator completed={done} className={active ? 'is-progress' : ''} /><QueueItemContent completed={done}>{toolActivity({ id: item.id, seq: 0, name: item.toolName, input: item.input, output: item.result, isError: item.status === 'failed', completed: done, awaitingUser: false })}</QueueItemContent><QueueItemActions><Badge variant={item.status === 'failed' ? 'destructive' : done ? 'secondary' : 'outline'}>{item.status === 'failed' ? '失败' : done ? '已完成' : active ? '进行中' : '待处理'}</Badge></QueueItemActions></div><QueueItemDescription completed={done}>{toolTarget(item)}</QueueItemDescription></QueueItem> })}</QueueList></Queue></div>
  }
  if (mode === 'browser') {
    return <WebPreview defaultUrl="https://fox.local/preview" className="fox-browser-panel"><div className="fox-browser-tabs"><button type="button" className="is-active"><Globe2 size={13} />预览</button><Plus size={14} /></div><WebPreviewNavigation className="fox-browser-address"><WebPreviewNavigationButton tooltip="刷新"><RotateCcw size={13} /></WebPreviewNavigationButton><WebPreviewUrl aria-label="预览地址" /><WebPreviewNavigationButton tooltip="在浏览器中打开" onClick={() => toast('浏览器能力将在后端接入阶段启用')}><ExternalLink size={13} /></WebPreviewNavigationButton></WebPreviewNavigation><div className="fox-browser-empty"><img className="fox-panel-mascot" src={mascotAssets.search} alt="" /><strong>浏览器预览尚未启动</strong><span>接入网页工具后，这里会显示 Agent 正在访问的页面。</span><Button size="sm" variant="outline" onClick={() => toast('浏览器能力将在后端接入阶段启用')}>启动预览</Button></div></WebPreview>
  }
  if (mode === 'agents') {
    return <AIAgent className="fox-agent-panel"><div className="fox-agent-summary"><div className="fox-agent-avatars"><span><img src={mascotAssets.laptop} alt="" /></span><span><img src={mascotAssets.search} alt="" /></span><span><Check size={11} /></span></div><div className="fox-agent-heading"><AIAgentHeader name="子 Agent" model="知识库服务" /><small>0 个运行中 · 1 个已完成</small></div></div><AIAgentContent className="fox-agent-content"><button type="button" className="fox-agent-create" onClick={() => toast('子智能体将复用知识库服务的编排能力')}><Plus size={15} />新建子任务</button><div className="fox-agent-section-label"><span>最近任务</span><small>1</small></div><button type="button" className="fox-agent-history" onClick={() => toast('子任务详情将在接入知识库服务后显示')}><span className="fox-agent-state"><Check size={12} /></span><p><strong>审查 Kun 工作台结构</strong><small>UI 结构、组件复用与响应式检查</small></p><span className="fox-agent-metrics"><strong>3 分钟</strong><small>7 步</small></span><ChevronRight size={14} /></button></AIAgentContent></AIAgent>
  }
  const labels: Record<Exclude<RightMode, 'files' | 'knowledge' | 'changes'>, [string, string]> = {
    todo: ['暂无待办', 'Fox 创建的待办会显示在这里'],
    browser: ['浏览器未启动', '网页预览会显示在这里'],
    agents: ['暂无子 Agent', '后续阶段将接入远程子智能体']
  }
  return <div className="fox-empty-panel"><ClipboardList size={29} /><strong>{labels[mode][0]}</strong><span>{labels[mode][1]}</span></div>
}

const rightItems: Array<[RightMode, string, ReactNode]> = [
  ['knowledge', '知识库', <BookOpen size={16} />],
  ['files', '文件', <Folders size={16} />],
  ['changes', '更改', <FileEdit size={16} />],
  ['todo', '待办', <ListTodo size={16} />],
  ['browser', '浏览器', <Globe2 size={16} />],
  ['agents', '子 Agent', <Bot size={16} />]
]

function RightPanel({ mode, detail, width, compact = false, onClose }: { mode: RightMode; detail: ConversationDetail | null; width: number; compact?: boolean; onClose: () => void }) {
  const item = rightItems.find(([id]) => id === mode)
  const detailLabel = mode === 'files'
    ? detail?.conversation.projectRoot?.split(/[\\/]/).at(-1) ?? '未选择项目'
    : mode === 'changes'
      ? `${detail?.toolCalls.filter((tool) => /write|edit|patch|create/i.test(tool.toolName)).length ?? 0} 个更改`
      : '当前会话'
  return (
    <aside className={`fox-context-panel ${compact ? 'is-compact' : ''}`} style={compact ? undefined : { width }}>
      <div className="fox-context-head"><IconButton label="关闭右侧面板" onClick={onClose}><PanelRightClose size={16} /></IconButton><div><strong>{item?.[1]}</strong><small>{detailLabel}</small></div></div>
      <ContextContent mode={mode} detail={detail} />
    </aside>
  )
}

function SideRail({ mode, changeCount, onMode, onCollapse, onSettings }: { mode: RightMode | null; changeCount: number; onMode: (mode: RightMode) => void; onCollapse: () => void; onSettings: () => void }) {
  return (
    <nav className="fox-side-rail" aria-label="工作台工具">
      <div className="fox-rail-slot"><IconButton label="收起右侧边栏" tooltipSide="left" onClick={onCollapse}><PanelRightClose size={16} /></IconButton></div>
      {rightItems.map(([id, label, icon]) => <div key={id} className="fox-rail-slot"><IconButton label={label} tooltipSide="left" active={mode === id} onClick={() => onMode(id)}>{icon}</IconButton>{id === 'changes' && changeCount > 0 && <span className="fox-rail-count">{changeCount}</span>}</div>)}
      <span />
      <div className="fox-rail-slot"><IconButton label="设置" tooltipSide="left" onClick={onSettings}><Settings size={16} /></IconButton></div>
    </nav>
  )
}

function ResizeDivider({ onResize }: { onResize: (delta: number) => void }) {
  const start = useRef<{ x: number } | null>(null)
  return (
    <div
      className="fox-resize-divider"
      role="separator"
      aria-label="调整右侧面板宽度"
      aria-orientation="vertical"
      onPointerDown={(event) => { start.current = { x: event.clientX }; event.currentTarget.setPointerCapture(event.pointerId) }}
      onPointerMove={(event) => { if (!start.current) return; const delta = start.current.x - event.clientX; start.current = { x: event.clientX }; onResize(delta) }}
      onPointerUp={() => { start.current = null }}
    ><span /></div>
  )
}

function SidebarResizeDivider({ onResize }: { onResize: (delta: number) => void }) {
  const start = useRef<{ x: number } | null>(null)
  return <div className="fox-sidebar-resize-divider" role="separator" aria-label="调整侧边栏宽度" aria-orientation="vertical" onPointerDown={(event) => { start.current = { x: event.clientX }; event.currentTarget.setPointerCapture(event.pointerId) }} onPointerMove={(event) => { if (!start.current) return; const delta = event.clientX - start.current.x; start.current = { x: event.clientX }; onResize(delta) }} onPointerUp={() => { start.current = null }}><span /></div>
}

function TerminalResizeDivider({ onResize }: { onResize: (delta: number) => void }) {
  const start = useRef<{ y: number } | null>(null)
  return <div className="fox-terminal-resize-divider" role="separator" aria-label="调整终端高度" aria-orientation="horizontal" onPointerDown={(event) => { start.current = { y: event.clientY }; event.currentTarget.setPointerCapture(event.pointerId) }} onPointerMove={(event) => { if (!start.current) return; const delta = start.current.y - event.clientY; start.current = { y: event.clientY }; onResize(delta) }} onPointerUp={() => { start.current = null }} />
}

function ChatTopbar({ collapsed, onCollapse, rightSidebarCollapsed, onRightSidebarExpand, title, agentName, dark, focusMode, terminalOpen, state, onDark, onFocusMode, onTerminal }: { collapsed: boolean; onCollapse: () => void; rightSidebarCollapsed: boolean; onRightSidebarExpand: () => void; title?: string; agentName?: string; dark: boolean; focusMode: boolean; terminalOpen: boolean; state: ChatState; onDark: () => void; onFocusMode: () => void; onTerminal: () => void }) {
  return (
    <header className="fox-chat-topbar">
      <div className="fox-workspace-heading">
        {!collapsed && <IconButton label="收起侧边栏" tooltipSide="bottom" onClick={onCollapse}><PanelLeftClose size={16} /></IconButton>}
        <div><strong>{title || '新对话'}</strong><span>Fox · {agentName || 'Agent'} · 刚刚</span></div>
      </div>
      <div className="fox-chat-top-actions">
        <DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="选择编辑器"><Code2 size={16} /></Button></DropdownMenuTrigger></TooltipTrigger>
            <TooltipContent side="bottom"><p>选择编辑器</p></TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="end" className="fox-editor-menu">
            <DropdownMenuLabel>在编辑器中打开</DropdownMenuLabel>
            <DropdownMenuItem><Code2 />Visual Studio Code<Check className="fox-menu-check" /></DropdownMenuItem>
            <DropdownMenuItem><Terminal />Cursor</DropdownMenuItem>
            <DropdownMenuItem><ExternalLink />系统默认应用</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <IconButton label={terminalOpen ? '关闭终端' : '打开终端'} tooltipSide="bottom" active={terminalOpen} onClick={onTerminal}><Terminal size={16} /></IconButton>
        {state === 'running' && <Badge className="fox-running-badge"><span />运行中</Badge>}
        {state === 'question' && <Badge className="fox-question-badge"><CircleHelp size={12} />等待回答</Badge>}
        {state === 'approval' && <Badge className="fox-approval-badge"><ShieldCheck size={12} />等待批准</Badge>}
        {state === 'error' && <Badge className="fox-error-badge"><AlertTriangle size={12} />连接错误</Badge>}
        {state === 'denied' && <Badge className="fox-denied-badge"><CircleStop size={12} />已取消</Badge>}
        <DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="更多"><MoreHorizontal size={16} /></Button></DropdownMenuTrigger></TooltipTrigger>
            <TooltipContent side="bottom"><p>更多</p></TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="end" className="fox-top-menu">
            <DropdownMenuLabel>当前对话</DropdownMenuLabel>
            <DropdownMenuItem onSelect={onFocusMode}><Maximize2 />{focusMode ? '退出专注模式' : '专注模式'}</DropdownMenuItem>
            <DropdownMenuItem onSelect={onDark}><SunMoon />{dark ? '切换到浅色' : '切换到深色'}</DropdownMenuItem>
            <DropdownMenuItem><UserRound />邀请协作</DropdownMenuItem>
            <DropdownMenuItem><Copy />复制链接</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem><Archive />归档对话</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        {rightSidebarCollapsed && <IconButton label="展开右侧边栏" tooltipSide="bottom" onClick={onRightSidebarExpand}><PanelRightOpen size={16} /></IconButton>}
      </div>
    </header>
  )
}

export function Workbench() {
  const desktopConversation = useDesktopConversation()
  const yuxi = useYuxiService()
  const yuxiUser = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const yuxiModels = useYuxiModels(Boolean(yuxi.service?.credentialConfigured))
  const modelService = useModelService()
  const agentResource = useAgents()
  const knowledge = useKnowledgeBases(Boolean(yuxi.service?.credentialConfigured))
  const [activeView, setActiveView] = useState<WorkspaceView>(() => typeof window !== 'undefined' && !window.localStorage.getItem('fox.onboarding.status') ? 'onboarding' : 'chat')
  const [assistantMode, setAssistantMode] = useState<AssistantMode>('assistant')
  const [activeEntityId, setActiveEntityId] = useState<string | null>(null)
  const [activeDocumentId, setActiveDocumentId] = useState<string | null>(null)
  const [activeSourceLocator, setActiveSourceLocator] = useState<KnowledgeSourceLocator | null>(null)
  const managementReturnRoute = useRef<{ view: WorkspaceView; entityId: string | null; documentId: string | null }>({ view: 'chat', entityId: null, documentId: null })
  const onboardingReturnRoute = useRef<{ view: WorkspaceView; entityId: string | null; documentId: string | null }>({ view: 'chat', entityId: null, documentId: null })
  useEffect(() => {
    void desktopConversation.refreshRuntimeStatus()
  }, [desktopConversation.refreshRuntimeStatus, modelService.service?.supportsImageInput])
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => readStoredBoolean(layoutStorage.sidebarCollapsed, false))
  const [sidebarWidth, setSidebarWidth] = useState(() => readStoredNumber(layoutStorage.sidebarWidth, 304, 280, 480))
  const [rightMode, setRightMode] = useState<RightMode | null>(readStoredRightMode)
  const [rightSidebarCollapsed, setRightSidebarCollapsed] = useState(() => readStoredBoolean(layoutStorage.rightSidebarCollapsed, false))
  const [rightWidth, setRightWidth] = useState(() => readStoredNumber(layoutStorage.rightWidth, 360, 280, 760))
  const [dark, setDark] = useState(() => typeof window !== 'undefined' && window.localStorage.getItem(layoutStorage.theme) === 'dark')
  const [focusMode, setFocusMode] = useState(() => readStoredBoolean(layoutStorage.focusMode, false))
  const [mascotIndex, setMascotIndex] = useState(0)
  const [mascotCelebrating, setMascotCelebrating] = useState(false)
  const [compactLayout, setCompactLayout] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 1180px)').matches)
  const [compactRightOpen, setCompactRightOpen] = useState(false)
  const [emptyConversation, setEmptyConversation] = useState(false)
  const [terminalOpen, setTerminalOpen] = useState(false)
  const [terminalHeight, setTerminalHeight] = useState(() => readStoredNumber(layoutStorage.terminalHeight, 250, 180, 720))
  const [composerHeight, setComposerHeight] = useState(180)
  const [projectDialogOpen, setProjectDialogOpen] = useState(false)
  const [projectDialogBusy, setProjectDialogBusy] = useState(false)
  const [projectDialogBrowsing, setProjectDialogBrowsing] = useState(false)
  const [projectDialogInitialRoot, setProjectDialogInitialRoot] = useState<string | null>(null)
  const [projectDialogError, setProjectDialogError] = useState<string | null>(null)
  const [knowledgeDialogOpen, setKnowledgeDialogOpen] = useState(false)
  const [knowledgeDialogBusy, setKnowledgeDialogBusy] = useState(false)
  const [knowledgeDialogError, setKnowledgeDialogError] = useState<string | null>(null)
  const [conversationDialog, setConversationDialog] = useState<{ conversation: { id: string; title: string }; mode: 'rename' | 'delete' } | null>(null)
  const [conversationDialogBusy, setConversationDialogBusy] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [chatState, setChatState] = useState<ChatState>('complete')
  const [activePrompt, setActivePrompt] = useState('Kun 的协议是什么协议，我可以拿来二次开发并且企业内部使用吗？')
  const [composerResetKey, setComposerResetKey] = useState(0)
  const [suggestedPrompt, setSuggestedPrompt] = useState('')
  const [pendingUserMessage, setPendingUserMessage] = useState<ConversationMessage | null>(null)
  const workflowTimer = useRef<number | null>(null)
  const mascotTimer = useRef<number | null>(null)
  const previousDesktopRunning = useRef(false)
  const desktopUserMessage = desktopConversation.detail ? latestMessage(desktopConversation.detail.messages, 'user') : undefined
  const desktopAssistantMessage = desktopConversation.detail ? latestMessage(desktopConversation.detail.messages, 'assistant') : undefined
  const desktopCurrentRunAssistantMessage = desktopConversation.detail?.lastRun
    ? [...desktopConversation.detail.messages].reverse().find((message) => (
        message.role === 'assistant' && message.runId === desktopConversation.detail?.lastRun?.id
      ))
    : undefined
  const desktopRunning = desktopConversation.enabled && runIsActive(desktopConversation.detail)
  const desktopRunStatus = desktopConversation.detail?.lastRun?.status
  const activeAgent = agentResource.agents.find((item) => item.id === desktopConversation.selectedAgentId) ?? null
  const activeProject = projects.find((item) => item.id === desktopConversation.detail?.conversation.projectId)
    ?? projects.find((item) => item.rootPath === desktopConversation.draftProjectRoot)
    ?? null
  const conversationUsage = useMemo(() => latestConversationUsage(desktopConversation.detail?.runtimeEvents ?? []), [desktopConversation.detail?.runtimeEvents])
  const runtimeQuestion = useMemo(() => pendingRuntimeQuestion(desktopConversation.detail), [desktopConversation.detail])
  const timelineAssistantName = desktopConversation.detail?.conversation.agentName ?? activeAgent?.name ?? 'Fox 默认助手'
  const timelineRunModel = desktopConversation.detail?.lastRun?.model
  const pendingRuntimeApproval = desktopConversation.detail?.approvals.find((approval) => approval.status === 'pending')
  const visiblePrompt = desktopConversation.enabled ? pendingUserMessage?.content ?? desktopUserMessage?.content ?? activePrompt : activePrompt
  const visibleReply = desktopConversation.enabled
    ? desktopConversation.streamingText || desktopCurrentRunAssistantMessage?.content || ''
    : undefined
  const activeRuntimeEvents = useMemo(() => {
    const runId = desktopConversation.detail?.lastRun?.id
    if (!runId) return []
    return desktopConversation.detail?.runtimeEvents.filter((event) => event.runId === runId) ?? []
  }, [desktopConversation.detail?.lastRun?.id, desktopConversation.detail?.runtimeEvents])
  const sidebarMascot = mascotCelebrating
    ? mascotAt(mascotLibrary.success, mascotIndex, mascotAssets.cheer)
    : desktopConversation.error
      ? mascotAt(mascotLibrary.error, mascotIndex, mascotAssets.offline)
      : chatState === 'question' || chatState === 'approval'
        ? mascotAt(mascotLibrary.waiting, mascotIndex, mascotAssets.idle)
      : desktopRunning
        ? mascotAt(workingMascots, mascotIndex, mascotAssets.work)
        : mascotAt(idleMascots, mascotIndex, mascotAssets.idle)

  useEffect(() => {
    if (!activeAgent) return
    setAssistantMode(activeAgent.runtimeType === 'yuxi' ? 'knowledge' : 'assistant')
  }, [activeAgent])

  useEffect(() => {
    document.documentElement.classList.toggle('dark', dark)
    window.localStorage.setItem(layoutStorage.theme, dark ? 'dark' : 'light')
  }, [dark])
  useEffect(() => { window.localStorage.setItem(layoutStorage.sidebarCollapsed, sidebarCollapsed ? '1' : '0') }, [sidebarCollapsed])
  useEffect(() => { window.localStorage.setItem(layoutStorage.sidebarWidth, String(Math.round(sidebarWidth))) }, [sidebarWidth])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightWidth, String(Math.round(rightWidth))) }, [rightWidth])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightMode, rightMode ?? 'closed') }, [rightMode])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightSidebarCollapsed, rightSidebarCollapsed ? '1' : '0') }, [rightSidebarCollapsed])
  useEffect(() => { window.localStorage.setItem(layoutStorage.focusMode, focusMode ? '1' : '0') }, [focusMode])
  useEffect(() => { window.localStorage.setItem(layoutStorage.terminalHeight, String(Math.round(terminalHeight))) }, [terminalHeight])
  useEffect(() => {
    if (!desktopConversation.enabled) return
    void desktopClient.listProjects().then(setProjects).catch(() => undefined)
  }, [desktopConversation.enabled, desktopConversation.detail?.conversation.projectId])
  useEffect(() => {
    if (mascotCelebrating) return
    const mascotPool = desktopRunning ? workingMascots : idleMascots
    const interval = window.setInterval(() => setMascotIndex((value) => {
      const offset = 1 + Math.floor(Math.random() * Math.max(1, mascotPool.length - 1))
      return (value + offset) % mascotPool.length
    }), 10000)
    return () => window.clearInterval(interval)
  }, [desktopRunning, mascotCelebrating])
  useEffect(() => {
    const wasRunning = previousDesktopRunning.current
    previousDesktopRunning.current = desktopRunning

    if (desktopRunning) {
      if (mascotTimer.current !== null) window.clearTimeout(mascotTimer.current)
      mascotTimer.current = null
      setMascotCelebrating(false)
      return
    }
    if (!wasRunning || desktopConversation.error || desktopRunStatus !== 'completed') return

    setMascotIndex(Math.floor(Math.random() * mascotLibrary.success.length))
    setMascotCelebrating(true)
    mascotTimer.current = window.setTimeout(() => {
      setMascotCelebrating(false)
      mascotTimer.current = null
    }, 2600)
  }, [desktopConversation.error, desktopRunning, desktopRunStatus])
  useEffect(() => {
    const query = window.matchMedia('(max-width: 1180px)')
    const syncLayout = () => {
      setCompactLayout(query.matches)
      if (!query.matches) setCompactRightOpen(false)
    }
    syncLayout()
    query.addEventListener('change', syncLayout)
    return () => query.removeEventListener('change', syncLayout)
  }, [])
  useEffect(() => () => {
    if (workflowTimer.current !== null) window.clearTimeout(workflowTimer.current)
    if (mascotTimer.current !== null) window.clearTimeout(mascotTimer.current)
  }, [])
  useEffect(() => {
    if (!desktopConversation.enabled) return
    if (pendingRuntimeApproval) setChatState('approval')
    else if (desktopConversation.error) setChatState('error')
    else if (desktopRunning) setChatState('running')
    else if (desktopAssistantMessage || desktopUserMessage) setChatState('complete')
  }, [desktopAssistantMessage, desktopConversation.enabled, desktopConversation.error, desktopRunning, desktopUserMessage, pendingRuntimeApproval])
  useEffect(() => {
    if (!pendingUserMessage || !desktopUserMessage) return
    if (desktopUserMessage.content !== pendingUserMessage.content) return
    if (desktopUserMessage.createdAt < pendingUserMessage.createdAt - 3000) return
    setPendingUserMessage(null)
  }, [desktopUserMessage, pendingUserMessage])

  const shellStyle = useMemo(() => ({ '--fox-sidebar-width': `${sidebarCollapsed ? 64 : sidebarWidth}px`, '--fox-composer-occlusion': `${composerHeight}px` } as React.CSSProperties), [composerHeight, sidebarCollapsed, sidebarWidth])

  const selectRightMode = (mode: RightMode) => {
    setRightSidebarCollapsed(false)
    if (compactLayout) {
      if (compactRightOpen && rightMode === mode) {
        setCompactRightOpen(false)
        setRightMode(null)
        return
      }
      setRightMode(mode)
      setCompactRightOpen(true)
      return
    }
    setRightMode((current) => current === mode ? null : mode)
  }
  const toggleFocusMode = (value = !focusMode) => {
    setFocusMode(value)
    if (value) setCompactRightOpen(false)
  }
  const clearWorkflowTimer = () => {
    if (workflowTimer.current === null) return
    window.clearTimeout(workflowTimer.current)
    workflowTimer.current = null
  }
  const finishWorkflow = (delay = 2200) => {
    clearWorkflowTimer()
    setChatState('running')
    workflowTimer.current = window.setTimeout(() => {
      setChatState('complete')
      workflowTimer.current = null
    }, delay)
  }
  const resetConversation = (suggestion = '') => {
    clearWorkflowTimer()
    if (desktopConversation.enabled) {
      void desktopConversation.createConversation()
    }
    setChatState('complete')
    setPendingUserMessage(null)
    setEmptyConversation(!suggestion)
    setSuggestedPrompt(suggestion)
    setComposerResetKey((value) => value + 1)
    setActiveEntityId(null)
    setActiveDocumentId(null)
    setActiveSourceLocator(null)
    setActiveView('chat')
  }
  const prepareDraft = (agentId: string, mode: AssistantMode, projectRoot?: string, permissionMode?: ProjectRecord['permissionMode'], knowledgeBases: KnowledgeBaseRecord[] = []) => {
    void desktopConversation.createConversationForAgent(agentId, projectRoot, permissionMode).then(async (created) => {
      if (!created) {
        toast.error(desktopConversation.error ?? '无法创建对话草稿')
        return
      }
      if (knowledgeBases.length && !await desktopConversation.setKnowledgeBindings(knowledgeBases)) {
        toast.error(desktopConversation.error ?? '无法为对话启用知识库')
        return
      }
      clearWorkflowTimer()
      setAssistantMode(mode)
      setChatState('complete')
      setPendingUserMessage(null)
      setEmptyConversation(true)
      setSuggestedPrompt('')
      setComposerResetKey((value) => value + 1)
      setActiveEntityId(null)
      setActiveDocumentId(null)
      setActiveSourceLocator(null)
      setActiveView('chat')
    })
  }
  const askKnowledgeBase = (knowledgeBase: KnowledgeBaseRecord) => {
    const knowledgeAgent = agentResource.agents.find((agent) => agent.runtimeType === 'yuxi' && agent.isDefault && agent.available)
      ?? agentResource.agents.find((agent) => agent.runtimeType === 'yuxi' && agent.available)
    if (!knowledgeAgent) {
      toast.error(yuxi.service?.credentialConfigured ? '知识库服务暂无可用的默认知识助手' : '请先登录知识库，再使用知识助手')
      return
    }
    prepareDraft(knowledgeAgent.id, 'knowledge', undefined, undefined, [knowledgeBase])
  }
  const switchAssistantMode = (mode: AssistantMode) => {
    if (mode === 'assistant') {
      const localAgent = agentResource.agents.find((agent) => agent.id === 'fox-general')
        ?? agentResource.agents.find((agent) => agent.runtimeType !== 'yuxi' && agent.isDefault)
        ?? agentResource.agents.find((agent) => agent.runtimeType !== 'yuxi')
      if (!localAgent) {
        toast.error('Fox 本地默认助手暂不可用')
        return
      }
      prepareDraft(localAgent.id, mode)
      return
    }
    const knowledgeAgent = agentResource.agents.find((agent) => agent.runtimeType === 'yuxi' && agent.isDefault)
      ?? agentResource.agents.find((agent) => agent.runtimeType === 'yuxi')
    if (!knowledgeAgent) {
      toast.error(yuxi.service?.credentialConfigured ? '知识库服务暂无可用的默认智能助手' : '请先登录知识库，再使用知识模式')
      return
    }
    prepareDraft(knowledgeAgent.id, mode)
  }
  const createProjectDraft = (requestedProjectRoot?: string) => {
    const projectRoot = requestedProjectRoot
      ?? desktopConversation.detail?.conversation.projectRoot
      ?? desktopConversation.draftProjectRoot
      ?? desktopConversation.conversations.find((conversation) => conversation.projectRoot)?.projectRoot
    const requestedProject = requestedProjectRoot
      ? projects.find((project) => project.rootPath.replace(/[\\/]+$/, '').toLocaleLowerCase() === requestedProjectRoot.replace(/[\\/]+$/, '').toLocaleLowerCase())
      : activeProject
    const permissionMode = requestedProject?.permissionMode ?? desktopConversation.draftPermissionMode ?? undefined
    if (!projectRoot) {
      openProjectDialog()
      return
    }
    const agentId = desktopConversation.selectedAgentId ?? (assistantMode === 'assistant' ? 'fox-general' : null)
    if (!agentId) {
      toast.error('请先选择一个智能体')
      return
    }
    prepareDraft(agentId, assistantMode, projectRoot, permissionMode)
  }
  const openProjectDialog = () => {
    setProjectDialogError(null)
    setProjectDialogInitialRoot(desktopConversation.detail?.conversation.projectRoot ?? desktopConversation.draftProjectRoot ?? null)
    setProjectDialogOpen(true)
    if (!desktopConversation.enabled) return
    void desktopClient.listProjects()
      .then(setProjects)
      .catch((cause) => setProjectDialogError(cause instanceof Error ? cause.message : String(cause)))
  }
  const pickProjectFolder = async () => {
    setProjectDialogBrowsing(true)
    setProjectDialogError(null)
    try {
      return await desktopClient.pickProjectFolder()
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause)
      setProjectDialogError(message)
      toast.error(message)
      return null
    } finally {
      setProjectDialogBrowsing(false)
    }
  }
  const addProject = async () => {
    const selected = await pickProjectFolder()
    if (!selected) return
    setProjectDialogInitialRoot(selected)
    setProjectDialogOpen(true)
    if (desktopConversation.enabled) void desktopClient.listProjects().then(setProjects).catch(() => undefined)
  }
  const authorizeProject = async (projectRoot: string, permissionMode: ProjectRecord['permissionMode']) => {
    setProjectDialogBusy(true)
    setProjectDialogError(null)
    const created = await desktopConversation.createConversation(projectRoot, permissionMode)
    setProjectDialogBusy(false)
    if (!created) {
      setProjectDialogError(desktopConversation.error ?? '无法创建项目会话，请检查文件夹路径')
      return
    }
    setProjectDialogOpen(false)
    setActiveView('chat')
    const modeLabel = { read_only: '只读', ask: '询问', allow: '允许' }[permissionMode]
    toast.success(`项目文件夹已加入对话草稿，执行方式：${modeLabel}`)
  }
  const saveKnowledgeBindings = async (items: KnowledgeBaseRecord[]) => {
    setKnowledgeDialogBusy(true)
    setKnowledgeDialogError(null)
    const saved = await desktopConversation.setKnowledgeBindings(items)
    setKnowledgeDialogBusy(false)
    if (!saved) {
      setKnowledgeDialogError(desktopConversation.error ?? '无法保存知识库选择')
      return
    }
    setKnowledgeDialogOpen(false)
    toast.success(items.length ? `已为当前会话启用 ${items.length} 个知识库` : '已清除当前会话的知识库')
  }
  const renameManagedConversation = async (title: string) => {
    if (!conversationDialog) return
    setConversationDialogBusy(true)
    const renamed = await desktopConversation.renameConversation(conversationDialog.conversation.id, title)
    setConversationDialogBusy(false)
    if (!renamed) return
    setConversationDialog(null)
    toast.success('对话已重命名')
  }
  const deleteManagedConversation = async () => {
    if (!conversationDialog) return
    setConversationDialogBusy(true)
    const deleted = await desktopConversation.deleteConversation(conversationDialog.conversation.id)
    setConversationDialogBusy(false)
    if (!deleted) return
    setConversationDialog(null)
    toast.success('对话已删除')
  }
  useEffect(() => {
    if (projectDialogOpen && desktopConversation.error) setProjectDialogError(desktopConversation.error)
  }, [desktopConversation.error, projectDialogOpen])
  const navigate: NavigateWorkspace = (view, entityId, context) => {
    if (view === 'onboarding' && activeView !== 'onboarding') {
      onboardingReturnRoute.current = { view: activeView, entityId: activeEntityId, documentId: activeDocumentId }
    }
    if (view === 'chat' && entityId && entityId !== desktopConversation.detail?.conversation.agentId) {
      void desktopConversation.createConversationForAgent(entityId).then((created) => {
        if (!created) toast.error(desktopConversation.error ?? '无法使用这个智能体')
      })
    }
    const managementSection = (value: WorkspaceView) => settingsWorkspaceViews.includes(value)
      ? 'settings'
      : value === 'knowledge' || value === 'knowledge-detail' || value === 'knowledge-graph'
        ? 'knowledge'
        : null
    if (activeView === 'onboarding' && view !== 'onboarding') {
      managementReturnRoute.current = { view: 'onboarding', entityId: null, documentId: null }
    } else if (managementSection(view) && managementSection(view) !== managementSection(activeView)) {
      managementReturnRoute.current = { view: activeView, entityId: activeEntityId, documentId: activeDocumentId }
    }
    if (entityId) setActiveEntityId(entityId)
    setActiveDocumentId(context?.documentId ?? null)
    setActiveSourceLocator(context?.sourceLocator ?? null)
    setActiveView(view)
    setTerminalOpen(false)
    setCompactRightOpen(false)
    setRightMode(null)
  }
  const exitManagement = () => {
    const previous = managementReturnRoute.current
    setActiveView(previous.view)
    setActiveEntityId(previous.entityId)
    setActiveDocumentId(previous.documentId)
    setActiveSourceLocator(null)
    setTerminalOpen(false)
    setCompactRightOpen(false)
    setRightMode(null)
  }
  const finishOnboarding = (status: 'completed' | 'skipped') => {
    window.localStorage.setItem('fox.onboarding.status', status)
    const target = onboardingReturnRoute.current
    setActiveView(target.view === 'onboarding' ? 'chat' : target.view)
    setActiveEntityId(target.entityId)
    setActiveDocumentId(target.documentId)
    setActiveSourceLocator(null)
    setTerminalOpen(false)
    setCompactRightOpen(false)
    setRightMode(null)
    if (status === 'skipped' && !modelService.service) toast('设置已跳过，配置模型服务后才能正常使用 Fox 助手')
  }
  useEffect(() => {
    const handleShortcut = (event: globalThis.KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey) return
      const key = event.key.toLocaleLowerCase()
      if (!['n', 'o', ',', 'b', 'j'].includes(key)) return
      event.preventDefault()
      if (key === 'n') switchAssistantMode(assistantMode)
      if (key === 'o') void addProject()
      if (key === ',') navigate('settings')
      if (key === 'b') setSidebarCollapsed((value) => !value)
      if (key === 'j') {
        if (activeView !== 'chat') navigate('chat')
        setTerminalOpen((value) => !value)
      }
    }
    window.addEventListener('keydown', handleShortcut)
    return () => window.removeEventListener('keydown', handleShortcut)
  })

  const rawWorkspacePage = activeView === 'agents' ? <AgentListPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'agent-detail' ? <AgentDetailPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} agentId={activeEntityId} section={activeDocumentId ?? 'home'} />
    : activeView === 'knowledge' ? <KnowledgeListPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'knowledge-detail' ? <KnowledgeDetailPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} onAskKnowledge={askKnowledgeBase} knowledgeId={activeEntityId} initialDocumentId={activeDocumentId} sourceLocator={activeSourceLocator} />
    : activeView === 'knowledge-graph' ? <KnowledgeGraphPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} knowledgeId={activeEntityId} />
    : activeView === 'settings' ? <SettingsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} dark={dark} onDark={() => setDark(!dark)} />
    : activeView === 'settings-ai' ? <AiSettingsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'settings-models' ? <ModelProvidersPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} />
    : activeView === 'settings-yuxi' ? <YuxiSettingsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'settings-projects' ? <ProjectPermissionsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} />
    : activeView === 'settings-conversation' ? <ConversationSettingsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} />
    : activeView === 'settings-usage' ? <UsageStatisticsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} />
    : activeView === 'settings-extensions' ? <ExtensionsSettingsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'settings-about' ? <AboutSettingsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} />
    : activeView === 'skills' ? <SkillsPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'mcp' ? <McpPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'maintenance' ? <MaintenancePage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'service' ? <ServicePage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'model-service' ? <ModelServicePage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : activeView === 'login' ? <LoginPage sidebarCollapsed={sidebarCollapsed || compactLayout} onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)} navigate={navigate} />
    : null
  const workspacePage = rawWorkspacePage
    ? <Suspense fallback={<div className="fox-route-loading"><span /><p>正在载入页面</p></div>}>{rawWorkspacePage}</Suspense>
    : null
  const timelineEmpty = desktopConversation.enabled ? !desktopUserMessage && !pendingUserMessage : emptyConversation
  const showConversationRightSidebar = activeView === 'chat'

  if (activeView === 'onboarding') return <Suspense fallback={<div className="fox-app-loading"><img src={mascotAssets.brand} alt="" /><p>Fox 正在准备引导页</p></div>}><OnboardingPage navigate={navigate} onExit={finishOnboarding} /></Suspense>

  return (
    <main className={`fox-shell ${focusMode ? 'is-focus-mode' : ''}`} style={shellStyle}>
      <WindowTitlebar onNewChat={() => switchAssistantMode(assistantMode)} onOpenProject={() => void addProject()} onSettings={() => navigate('settings')} onToggleSidebar={() => setSidebarCollapsed((value) => !value)} onToggleTerminal={() => { if (activeView !== 'chat') navigate('chat'); setTerminalOpen((value) => !value) }} onZoom={(delta) => { const current = Number(document.documentElement.dataset.foxZoom ?? '1'); const next = Math.min(1.4, Math.max(.8, Math.round((current + delta) * 10) / 10)); document.documentElement.dataset.foxZoom = String(next); document.documentElement.style.zoom = String(next) }} />
      <div className="fox-workbench">
        <Sidebar collapsed={sidebarCollapsed || compactLayout} assistantMode={assistantMode} onCollapse={() => setSidebarCollapsed(!sidebarCollapsed)} onModeChange={switchAssistantMode} onNewChat={() => switchAssistantMode(assistantMode)} onNewProjectChat={createProjectDraft} onAddProject={() => void addProject()} onOpenConversation={(conversationId) => { setPendingUserMessage(null); if (desktopConversation.enabled && conversationId) void desktopConversation.openConversation(conversationId); setEmptyConversation(false); navigate('chat') }} onRenameConversation={(conversation) => setConversationDialog({ conversation, mode: 'rename' })} onDeleteConversation={(conversation) => setConversationDialog({ conversation, mode: 'delete' })} onNavigate={navigate} onExitManagement={exitManagement} onTheme={() => setDark(!dark)} onSettings={() => navigate('settings')} runtimeConversations={desktopConversation.enabled ? desktopConversation.conversations : undefined} onSearchConversations={desktopConversation.enabled ? desktopConversation.searchConversations : undefined} activeConversationId={desktopConversation.detail?.conversation.id} yuxiService={yuxi.service ? { name: yuxi.service.name, status: yuxi.service.lastStatus, connectionType: yuxi.service.connectionType } : null} yuxiUser={yuxiUser.user} activeView={activeView} activeEntityId={activeEntityId} activeDocumentId={activeDocumentId} />
        {!sidebarCollapsed && !compactLayout && <SidebarResizeDivider onResize={(delta) => setSidebarWidth((value) => Math.min(480, Math.max(280, value + delta)))} />}
        <div className="fox-content-card">
        <div className="fox-content-surface">
        <section className="fox-chat-pane">
          <ComposerRuntimeContext.Provider value={{ knowledgeBindings: desktopConversation.knowledgeBindings, assistantName: timelineAssistantName, runModel: timelineRunModel, onOpenSource: (source) => navigate('knowledge-detail', source.knowledgeBaseId, { documentId: source.documentId, sourceLocator: source.locator }), questionRequest: runtimeQuestion, onSubmitQuestion: async (text, answers) => { if (!runtimeQuestion) return false; clearWorkflowTimer(); setActivePrompt(text); setChatState('running'); const started = await desktopConversation.resumeQuestion(runtimeQuestion.runId, text, answers); if (!started) { setChatState('error'); return false } return true }, onKnowledge: () => { setKnowledgeDialogError(null); setKnowledgeDialogOpen(true); void knowledge.refresh() } }}>
          {workspacePage ?? <><ChatTopbar collapsed={sidebarCollapsed || compactLayout} onCollapse={() => setSidebarCollapsed(!sidebarCollapsed)} rightSidebarCollapsed={rightSidebarCollapsed} onRightSidebarExpand={() => setRightSidebarCollapsed(false)} title={desktopConversation.detail?.conversation.title} agentName={desktopConversation.detail?.conversation.agentName ?? activeAgent?.name} dark={dark} focusMode={focusMode} terminalOpen={terminalOpen} state={chatState} onDark={() => setDark(!dark)} onFocusMode={() => toggleFocusMode()} onTerminal={() => setTerminalOpen(!terminalOpen)} />
          <div className={`fox-chat-stage ${timelineEmpty ? 'is-empty' : ''}`}><Timeline hasEarlierMessages={desktopConversation.detail?.hasEarlierMessages} loadingEarlierMessages={desktopConversation.loadingEarlierMessages} onLoadEarlierMessages={() => void desktopConversation.loadEarlierMessages()} empty={timelineEmpty} state={chatState} prompt={visiblePrompt} mascotSrc={sidebarMascot} mascotActive={desktopRunning || mascotCelebrating} runtimeMessages={desktopConversation.enabled ? desktopConversation.detail?.messages ?? [] : undefined} runtimeAttachments={desktopConversation.enabled ? desktopConversation.detail?.attachments ?? [] : undefined} runtimeArtifacts={desktopConversation.enabled ? desktopConversation.detail?.artifacts ?? [] : undefined} pendingMessage={pendingUserMessage} runtimeEvents={desktopConversation.enabled ? desktopConversation.detail?.runtimeEvents : undefined} runtimeApprovals={desktopConversation.enabled ? desktopConversation.detail?.approvals : undefined} runtimeRunId={desktopConversation.detail?.lastRun?.id} runtimeRunning={desktopRunning} runtimeReply={visibleReply} runtimeError={desktopConversation.error} onApprove={() => finishWorkflow()} onDeny={() => { clearWorkflowTimer(); setChatState('denied') }} onRetry={() => desktopConversation.enabled && visiblePrompt ? void desktopConversation.send(visiblePrompt) : finishWorkflow()} onAnswer={(answer) => { toast.success(`已选择：${answer}`); finishWorkflow(2600) }} onStart={(suggestion) => resetConversation(suggestion)} onResolveApproval={(approvalId, approved) => void desktopConversation.resolveApproval(approvalId, approved)} /><Composer resetKey={composerResetKey} suggestedPrompt={suggestedPrompt} chatState={chatState} centered={timelineEmpty} runtimeControlled={desktopConversation.enabled} projectRoot={desktopConversation.detail?.conversation.projectRoot ?? desktopConversation.draftProjectRoot} projectPermissionMode={activeProject?.permissionMode ?? desktopConversation.draftPermissionMode} activeAgent={activeAgent} agents={agentResource.agents} modelService={modelService.service} runtimeCapabilities={desktopConversation.runtimeStatus?.capabilities} yuxiModels={yuxiModels.models} usage={conversationUsage} mascotSrc={timelineEmpty ? undefined : sidebarMascot} mascotActive={desktopRunning || mascotCelebrating} onProject={openProjectDialog} onAgentChange={(agentId) => { void desktopConversation.createConversationForAgent(agentId).then((created) => { if (created) toast.success('已切换智能体，发送消息后创建对话') }) }} onHeightChange={setComposerHeight} onPromptCommit={(prompt) => { const now = Date.now(); setActivePrompt(prompt); setSuggestedPrompt(''); setEmptyConversation(false); setChatState('running'); setPendingUserMessage({ id: `ui-pending-${now}`, conversationId: desktopConversation.detail?.conversation.id ?? 'pending', runId: null, role: 'user', kind: 'text', content: prompt, status: 'sending', ordinal: (desktopConversation.detail?.messages.at(-1)?.ordinal ?? 0) + 1, createdAt: now, updatedAt: now }) }} onSubmitPrompt={async (prompt, model, submittedFiles) => { clearWorkflowTimer(); setActivePrompt(prompt); setSuggestedPrompt(''); setEmptyConversation(false); if (desktopConversation.enabled) { setChatState('running'); const started = await desktopConversation.send(prompt, model, submittedFiles); if (!started) { setPendingUserMessage((message) => message ? { ...message, status: 'failed', updatedAt: Date.now() } : message); setChatState('error'); return 'error' } return 'complete' } if (/询问我|让我选择|需要确认方案|怎么处理/.test(prompt)) { setChatState('question'); return 'question' } if (/修改|写入|删除|重命名|创建文件/.test(prompt)) { setChatState('approval'); return 'approval' } if (/失败|错误|连接测试|检查连接/.test(prompt)) { setChatState('error'); return 'error' } setChatState('running'); return 'complete' }} onStatusChange={(status) => { if (!desktopConversation.enabled && status === 'ready' && chatState === 'running') setChatState('complete') }} onCancel={desktopConversation.enabled ? desktopConversation.cancel : undefined} /></div>
          {terminalOpen && <><TerminalResizeDivider onResize={(delta) => setTerminalHeight((value) => Math.min(720, Math.max(180, value + delta)))} /><AITerminal output={terminalOutput} isStreaming className="fox-terminal-drawer" style={{ height: terminalHeight }}><AITerminalHeader className="fox-terminal-head"><AITerminalTitle className="fox-terminal-title">powershell</AITerminalTitle><div className="fox-terminal-head-actions"><AITerminalStatus className="fox-terminal-status"><i />已连接</AITerminalStatus><AITerminalActions><AITerminalCopyButton className="fox-terminal-action" onCopy={() => toast.success('已复制终端输出')} /><IconButton label="关闭终端" onClick={() => setTerminalOpen(false)}><X size={14} /></IconButton></AITerminalActions></div></AITerminalHeader><AITerminalContent className="fox-terminal-content" /></AITerminal></>}</>}
          </ComposerRuntimeContext.Provider>
        </section>
        {showConversationRightSidebar && rightMode && !rightSidebarCollapsed && !compactLayout && <><ResizeDivider onResize={(delta) => setRightWidth((value) => Math.min(760, Math.max(280, value + delta)))} /><RightPanel mode={rightMode} detail={desktopConversation.detail} width={rightWidth} onClose={() => setRightMode(null)} /></>}
        {showConversationRightSidebar && !rightSidebarCollapsed && <SideRail mode={compactLayout && !compactRightOpen ? null : rightMode} changeCount={desktopConversation.detail?.toolCalls.filter((tool) => /write|edit|patch|create/i.test(tool.toolName)).length ?? 0} onMode={selectRightMode} onCollapse={() => { setRightSidebarCollapsed(true); setCompactRightOpen(false) }} onSettings={() => navigate('settings')} />}
        </div>
        </div>
      </div>
      <Sheet open={showConversationRightSidebar && !rightSidebarCollapsed && compactLayout && compactRightOpen && Boolean(rightMode)} onOpenChange={(open) => { setCompactRightOpen(open); if (!open && compactLayout) setRightMode(null) }}>
        <SheetContent side="right" showCloseButton={false} className="fox-compact-context-sheet">
          {rightMode && <RightPanel compact mode={rightMode} detail={desktopConversation.detail} width={rightWidth} onClose={() => { setCompactRightOpen(false); setRightMode(null) }} />}
        </SheetContent>
      </Sheet>
      <ProjectAccessDialog open={projectDialogOpen} currentRoot={projectDialogInitialRoot} projects={projects} busy={projectDialogBusy} browsing={projectDialogBrowsing} error={projectDialogError} onOpenChange={setProjectDialogOpen} onBrowse={pickProjectFolder} onConfirm={(path, permissionMode) => void authorizeProject(path, permissionMode)} />
      <KnowledgeBindingDialog open={knowledgeDialogOpen} items={knowledge.items} bindings={desktopConversation.knowledgeBindings} busy={knowledgeDialogBusy || knowledge.loading} error={knowledgeDialogError ?? knowledge.error} onOpenChange={setKnowledgeDialogOpen} onConfirm={(items) => void saveKnowledgeBindings(items)} />
      <ConversationManagementDialogs conversation={conversationDialog?.conversation ?? null} mode={conversationDialog?.mode ?? null} busy={conversationDialogBusy} onClose={() => setConversationDialog(null)} onRename={(title) => void renameManagedConversation(title)} onDelete={() => void deleteManagedConversation()} />
    </main>
  )
}
