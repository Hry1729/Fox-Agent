import { UserMessageAvatar, UserMessageBubble } from './components/UserMessageBubble'
import { ExpertPickerDialog } from '@/features/agents/ExpertPickerDialog'
import { HtmlFilePreview } from './html-file-preview'
import { KernelReconciliationPanel } from './kernel-reconciliation-panel'
import { runtimeProcessActivity } from './runtime-process-activity'
import { lazy, memo, Profiler, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction, type ChangeEvent, type KeyboardEvent, type ReactNode } from 'react'
import { subscribeNotificationRefresh } from '../notification-subscriptions'
import {
  Activity,
  Archive,
  AlertTriangle,
  ArrowLeft,
  ArrowUp,
  Bell,
  BookOpen,
  BookCopy,
  Bot,
  Brain,
  Check,
  ChevronDown,
  ChevronUp,
  ChevronRight,
  CircleStop,
  ClipboardList,
  Copy,
  Eraser,
  ExternalLink,
  File,
  FileEdit,
  FilePlus2,
  FileText,
  Folder,
  FolderOpen,
  Folders,
  GitFork,
  Globe2,
  House,
  CircleHelp,
  CircleDot,
  ImagePlus,
  Library,
  ListTodo,
  LoaderCircle,
  MessageCircleMore,
  MessageSquarePlus,
  Maximize2,
  Minus,
  Minimize2,
  MoreHorizontal,
  PackagePlus,
  PanelLeft,
  PanelRight,
  Pause,
  Paperclip,
  Pin,
  PinOff,
  Play,
  Plus,
  RotateCcw,
  Search,
  Server,
  Settings,
  Settings2,
  ShieldCheck,
  Sparkles,
  Square,
  Target,
  SunMoon,
  Terminal,
  Trash2,
  ThumbsDown,
  ThumbsUp,
  UserRound,
  UserRoundCheck,
  WandSparkles,
  Wrench,
  X
} from 'lucide-react'
import { createContext, useContext } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { flushSync } from 'react-dom'
import { BorderBeam } from 'border-beam'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable, knowledgeReferenceFromLegacyBinding, knowledgeReferenceKey } from '@/features/conversations/api/desktop-client'
import { normalizeProjectPermission, selectProjectRoot, validPickedProjectFolder } from './project-access-dialog-state'
import { filterKnowledgePickerItems, localKnowledgePickerState } from './knowledge-picker-state'
import { Button } from '@/components/ui/button'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Sheet, SheetContent } from '@/components/ui/sheet'
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
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
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger
} from '@/components/ui/context-menu'
import {
  Conversation,
  ConversationAutoScroll,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton,
  ConversationViewportState
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
  Queue,
  QueueItem,
  QueueItemActions,
  QueueItemContent,
  QueueItemDescription,
  QueueItemIndicator,
  QueueList
} from '@/components/ai-elements/queue'
import {
  WebPreview,
  WebPreviewBody,
  WebPreviewNavigation,
  WebPreviewNavigationButton
} from '@/components/ai-elements/web-preview'
import { Agent as AIAgent, AgentContent as AIAgentContent } from '@/components/ai-elements/agent'
import {
  PromptInput,
  PromptInputActionAddAttachments,
  PromptInputActionMenu,
  PromptInputActionMenuContent,
  PromptInputActionMenuItem,
  PromptInputActionMenuTrigger,
  PromptInputBody,
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
import { chatSelectableAgents, normalizeAgentClassification, resolveActiveChatAgent } from '@/features/agents/agent-classification'
import { conversationModelOverride, yuxiModelOverrideAllowed } from './model-selection'
import { resolveExpertKnowledgeDeclaration } from '@/features/agents/expert-knowledge'
import { useKnowledgeBases } from '@/features/knowledge/use-knowledge'
import { latestMessage, runIsActive, useDesktopConversation } from '@/features/conversations/hooks/use-desktop-conversation'
import { conversationRunState } from '@/features/conversations/model/kernel-snapshot'
import { pendingRuntimeQuestion } from '@/features/conversations/model/pending-interactions'
import { allowedApprovalDecisions, isRepairOverrideApproval, repairOverrideApprovalDetails, resolveAllowedApprovalDecision } from '@/features/conversations/model/approval-decision-policy'
import type { RuntimeQuestion, RuntimeQuestionRequest } from '@/features/conversations/model/pending-interactions'
import type { AgentRecord, AppNotificationRecord, ApprovalDecision, ApprovalRecord, ArtifactActionResponse, ArtifactApplication, ArtifactRecord, ArtifactInspectResponse, AttachmentRecord, ChildRunRecord, ConversationDetail, ConversationMessage, ConversationSummary, DesktopErrorDetails, GlobalSearchRecord, KnowledgeBaseRecord, KnowledgeBindingRecord, KnowledgeReference, ModelServiceRecord, NotificationPreferencesRecord, ProjectFileActionResponse, ProjectFileEntry, ProjectFilePreview, ProjectRecord, RunEventRecord, TaskEvidenceRecord, ToolCallRecord, YuxiModelRecord, YuxiUserRecord } from '@/features/conversations/model/types'
import {
  FOX_IN_APP_NOTIFICATION_EVENT,
  FOX_NOTIFICATIONS_CHANGED_EVENT,
  FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT,
  notify as toast,
  normalizeNotificationSoundId,
  notificationIsVisibleInContext,
  playNotificationSound,
  type InAppNotificationEventDetail,
} from '@/features/notifications'
import type { LocalKnowledgeBaseDto } from '@/features/conversations/api/desktop-client'
import type { KnowledgeSourceLocator, NavigateWorkspace, WorkspaceView } from '@/features/workspace/types'
import { captureManagementReturnRoutes, managementExitRoute, SETTINGS_WORKSPACE_VIEWS, type ManagementReturnRoutes } from '@/features/workspace/management-navigation'
import { WorkspaceShell } from '@/features/workspace/workspace-shell'
import { LocalKnowledgeSidebarNavigation } from '@/features/local-knowledge/local-knowledge-sidebar'
import { FOX_ASSISTANT_AVATAR, FoxAssistantAvatar, RunStatusText } from './components/FoxAssistantAvatar'
import { ReasoningText } from './components/ReasoningText'
import { NewConversationMascot } from './components/NewConversationMascot'
import { GoalProgress } from './components/GoalProgress'
import { ExpertActivationCard, ExpertBindingChip, type ExpertBindingView } from './components/ExpertBindingChip'
import { expertErrorMessage, expertErrorPresentation, expertInteractionLocked, latestExpertToolAvailability, mergeExpertBindingsIntoTimeline, type ExpertToolAvailability } from './components/expert-binding-ui'
import { useGoalProgress, type GoalProgressData } from './hooks/use-goal-progress'
import type { MessageResponseProps } from '@/components/ai-elements/message-response'
import { normalizeAssistantMarkdown } from '@/features/conversations/model/assistant-presentation'
import { UserProfileDialog, useUserProfile, type UserProfile } from '@/features/profile/user-profile'
import { childRunIsActive, childRunStatusLabel, formatDurationMs, isWebToolCall, latestConversationContextUsage, normalizeBrowserUrl, webActivitySummary, type ConversationUsage } from './sidebar-model'
import { EMPTY_RUNTIME_ARTIFACTS, EMPTY_RUNTIME_EVENTS, groupRuntimeRecords, latestRunAssistantId, groupAssistantContinuations } from './runtime-timeline-performance'
import { approvalPresentation } from '../conversations/model/approval-presentation'

const OnboardingPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.OnboardingPage })))
const MessageResponse = lazy(() => import('@/components/ai-elements/message-response').then((module) => ({ default: module.MessageResponse })))

const LOCAL_KNOWLEDGE_WORKSPACE_VIEWS: WorkspaceView[] = [
  'local-knowledge-home',
  'local-files',
  'local-knowledge',
  'local-knowledge-detail',
  'local-knowledge-documents',
  'local-knowledge-import',
  'local-knowledge-jobs',
  'local-knowledge-models',
  'local-knowledge-retrieval',
]

function MarkdownResponse({ children, ...props }: MessageResponseProps) {
  const normalizedChildren = typeof children === 'string' ? normalizeAssistantMarkdown(children) : children
  return (
    <Suspense fallback={<div className="fox-markdown-loading">{typeof children === 'string' ? children : ''}</div>}>
      <MessageResponse {...props}>{normalizedChildren}</MessageResponse>
    </Suspense>
  )
}


const layoutStorage = {
  sidebarCollapsed: 'fox.layout.sidebarCollapsed',
  sidebarWidth: 'fox.layout.sidebarWidth',
  rightMode: 'fox.layout.rightMode',
  rightTabs: 'fox.layout.rightTabs',
  rightSidebarCollapsed: 'fox.layout.rightSidebarCollapsed',
  rightWidth: 'fox.layout.rightWidth',
  theme: 'fox.theme',
  seenChanges: 'fox.layout.seenChanges'
} as const

// In the profile build, lazily load the telemetry collector and forward React
// Profiler commits to it. The dynamic import sits behind a literal-false flag in
// normal releases, so Rollup DCEs the whole branch and no telemetry chunk or
// runtime cost ships to production.
type RegionRenderHandler = (
  id: string,
  phase: 'mount' | 'update' | 'nested-update',
  actualDuration: number,
  baseDuration: number,
  startTime: number,
  commitTime: number,
) => void
let profilingOnRender: RegionRenderHandler | null = null
if (typeof __FOX_PROFILING__ !== 'undefined' && __FOX_PROFILING__) {
  void import('@/features/profiling/telemetry').then((module) => {
    profilingOnRender = module.profilingCollector.onRender
  })
}

function recordRegionRender(
  id: string,
  phase: 'mount' | 'update' | 'nested-update',
  actualDuration: number,
  baseDuration: number,
  startTime: number,
  commitTime: number,
) {
  if (typeof performance === 'undefined') return
  if (import.meta.env.DEV) {
    performance.mark(`fox.${id}.${phase}`, { detail: { actualDuration, baseDuration } })
  }
  profilingOnRender?.(id, phase, actualDuration, baseDuration, startTime, commitTime)
}

function useStableCallback<TArgs extends unknown[], TResult>(callback: (...args: TArgs) => TResult) {
  const callbackRef = useRef(callback)
  useLayoutEffect(() => {
    callbackRef.current = callback
  }, [callback])
  return useCallback((...args: TArgs) => callbackRef.current(...args), [])
}

const EMPTY_CONVERSATION_MESSAGES: ConversationMessage[] = []
const EMPTY_CONVERSATION_ATTACHMENTS: AttachmentRecord[] = []
const EMPTY_RUNTIME_APPROVALS: ApprovalRecord[] = []

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
interface OpenFileTab {
  id: string
  path: string
  name: string
  artifactId: string | null
}
interface ChildAgentSelection {
  run: ChildRunRecord
  avatarIndex: number
}
type ChatState = 'complete' | 'running' | 'question' | 'approval' | 'error' | 'denied'
type AssistantMode = 'assistant' | 'knowledge'
const CONVERSATION_FORK_EVENT = 'fox:conversation-fork'

function requestConversationFork(messageId: string) {
  window.dispatchEvent(new CustomEvent(CONVERSATION_FORK_EVENT, { detail: { messageId } }))
}

function isConfirmationQuestion(question: RuntimeQuestion) {
  if (question.options.length !== 2 || question.multiSelect) return false
  return question.options.every((option) => /确认|取消|同意|拒绝|继续|停止|是|否|yes|no/i.test(option.label))
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

function readDefaultProjectPermission(): ProjectRecord['permissionMode'] {
  return normalizeProjectPermission(
    typeof window === 'undefined' ? null : window.localStorage.getItem('fox.preferences.defaultPermission'),
  )
}

function readStoredNumber(key: string, fallback: number, min: number, max: number) {
  if (typeof window === 'undefined') return fallback
  const value = Number(window.localStorage.getItem(key))
  return Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback
}

function readStoredRightMode(): RightMode | null {
  if (typeof window === 'undefined') return 'files'
  const value = window.localStorage.getItem(layoutStorage.rightMode)
  if (value === 'results') return 'files'
  return value === 'todo' || value === 'changes' || value === 'browser' || value === 'files' || value === 'knowledge' || value === 'agents'
    ? value
    : value === 'closed' ? null : 'files'
}

function readStoredRightTabs(): RightMode[] {
  if (typeof window === 'undefined') return ['files']
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(layoutStorage.rightTabs) ?? 'null')
    if (Array.isArray(value)) {
      return [...new Set(value.filter((item): item is RightMode => (
        item === 'todo' || item === 'changes' || item === 'browser' || item === 'files' || item === 'knowledge' || item === 'agents'
      )))]
    }
  } catch {
    // Fall back to the previously persisted single-page state.
  }
  const mode = readStoredRightMode()
  return mode ? [mode] : []
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
function globalSearchKindLabel(kind: GlobalSearchRecord['kind']): string {
  return ({
    conversation: '对话', project: '项目', agent: '专家', digital_colleague: '数字同事',
    knowledge_base: '知识库', knowledge_document: '文档', task: '任务', artifact: '产物',
  })[kind]
}

const NOTIFICATION_TOAST_VISIBLE_MS = 5000
const NOTIFICATION_TOAST_EXIT_MS = 280
const NOTIFICATION_PIN_STORAGE_KEY = 'fox.notification.manually-pinned-ids'

interface NotificationToastEntry {
  instanceId: string
  notification: Pick<AppNotificationRecord, 'id' | 'kind' | 'title' | 'body'>
  leaving: boolean
}

function notificationFingerprint(item: AppNotificationRecord): string {
  return JSON.stringify([item.kind, item.severity, item.title, item.body, item.progress, item.status])
}

function notificationDay(item: AppNotificationRecord): { key: string; label: string } {
  const date = new Date(item.updatedAt)
  const today = new Date()
  const yesterday = new Date(today)
  yesterday.setDate(today.getDate() - 1)
  const key = `${date.getFullYear()}-${date.getMonth() + 1}-${date.getDate()}`
  const isSameDay = (value: Date) => date.getFullYear() === value.getFullYear()
    && date.getMonth() === value.getMonth()
    && date.getDate() === value.getDate()
  if (isSameDay(today)) return { key, label: '今天' }
  if (isSameDay(yesterday)) return { key, label: '昨天' }
  return { key, label: date.toLocaleDateString('zh-CN', { month: 'long', day: 'numeric', weekday: 'short' }) }
}

function loadManuallyPinnedNotificationIds(): Set<string> {
  if (typeof window === 'undefined') return new Set()
  try {
    const value = JSON.parse(window.localStorage.getItem(NOTIFICATION_PIN_STORAGE_KEY) ?? '[]')
    return new Set(Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [])
  } catch {
    return new Set()
  }
}

function isAutomaticPriorityNotification(item: AppNotificationRecord): boolean {
  return item.severity === 'high' || item.kind === 'approval' || item.kind === 'question' || item.kind === 'progress'
}

function NotificationCenter({
  onNavigate,
  open,
  onOpenChange,
  showTrigger,
  activeView,
  activeEntityId,
  activeConversationId,
}: {
  onNavigate: NavigateWorkspace
  open: boolean
  onOpenChange: (open: boolean) => void
  showTrigger: boolean
  activeView: WorkspaceView
  activeEntityId?: string | null
  activeConversationId?: string | null
}) {
  const [unreadOnly, setUnreadOnly] = useState(false)
  const [items, setItems] = useState<AppNotificationRecord[]>([])
  const [preferences, setPreferences] = useState<NotificationPreferencesRecord | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [toastItems, setToastItems] = useState<NotificationToastEntry[]>([])
  const [expandedId, setExpandedId] = useState<string | null>(null)
  const [manuallyPinnedIds, setManuallyPinnedIds] = useState<Set<string>>(loadManuallyPinnedNotificationIds)
  const observedNotifications = useRef<Map<string, string> | null>(null)
  const loadGeneration = useRef(0)
  const toastTimers = useRef(new Map<string, { leave: number; remove: number }>())

  const removeToast = useCallback((instanceId: string) => {
    const timers = toastTimers.current.get(instanceId)
    if (timers) {
      window.clearTimeout(timers.leave)
      window.clearTimeout(timers.remove)
      toastTimers.current.delete(instanceId)
    }
    setToastItems((current) => current.filter((item) => item.instanceId !== instanceId))
  }, [])

  const enqueueToast = useCallback((notification: NotificationToastEntry['notification']) => {
    const instanceId = `${notification.id}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`
    setToastItems((current) => {
      const discarded = current.slice(0, -2)
      for (const item of discarded) {
        const timers = toastTimers.current.get(item.instanceId)
        if (timers) {
          window.clearTimeout(timers.leave)
          window.clearTimeout(timers.remove)
          toastTimers.current.delete(item.instanceId)
        }
      }
      return [...current.slice(-2), { instanceId, notification, leaving: false }]
    })
    const leave = window.setTimeout(() => {
      setToastItems((current) => current.map((item) => item.instanceId === instanceId ? { ...item, leaving: true } : item))
    }, NOTIFICATION_TOAST_VISIBLE_MS)
    const remove = window.setTimeout(() => removeToast(instanceId), NOTIFICATION_TOAST_VISIBLE_MS + NOTIFICATION_TOAST_EXIT_MS)
    toastTimers.current.set(instanceId, { leave, remove })
  }, [removeToast])

  const clearToastQueue = useCallback(() => {
    for (const timers of toastTimers.current.values()) {
      window.clearTimeout(timers.leave)
      window.clearTimeout(timers.remove)
    }
    toastTimers.current.clear()
    setToastItems([])
  }, [])

  useEffect(() => {
    const handlePreview = (event: Event) => {
      const detail = (event as CustomEvent<InAppNotificationEventDetail>).detail
      if (!detail?.notification) return
      enqueueToast(detail.notification)
      if (detail.forceSound) playNotificationSound(normalizeNotificationSoundId(detail.soundId))
    }
    const handlePreferences = (event: Event) => {
      setPreferences((event as CustomEvent<NotificationPreferencesRecord>).detail)
    }
    window.addEventListener(FOX_IN_APP_NOTIFICATION_EVENT, handlePreview)
    window.addEventListener(FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, handlePreferences)
    return () => {
      window.removeEventListener(FOX_IN_APP_NOTIFICATION_EVENT, handlePreview)
      window.removeEventListener(FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, handlePreferences)
      for (const timers of toastTimers.current.values()) {
        window.clearTimeout(timers.leave)
        window.clearTimeout(timers.remove)
      }
      toastTimers.current.clear()
    }
  }, [enqueueToast])

  const load = useCallback(async (quiet = false) => {
    const generation = ++loadGeneration.current
    if (!desktopRuntimeAvailable) {
      setItems([])
      setError(null)
      return
    }
    if (!quiet) setLoading(true)
    try {
      const [nextItems, nextPreferences] = await Promise.all([
        desktopClient.listAppNotifications(unreadOnly, 100),
        desktopClient.getNotificationPreferences(),
      ])
      if (generation !== loadGeneration.current) return
      const visibleUnreadIds = new Set(nextItems
        .filter((item) => item.readAt == null && notificationIsVisibleInContext(item, {
          appVisible: document.visibilityState === 'visible',
          appFocused: document.hasFocus(),
          activeView,
          activeEntityId,
          activeConversationId,
        }))
        .map((item) => item.id))
      if (visibleUnreadIds.size) {
        await Promise.allSettled([...visibleUnreadIds].map((id) => desktopClient.setAppNotificationRead(id, true)))
      }
      if (generation !== loadGeneration.current) return
      const seenAt = Date.now()
      const presentedItems = nextItems.map((item) => visibleUnreadIds.has(item.id) ? { ...item, readAt: seenAt } : item)
      const nextObserved = new Map(presentedItems.map((item) => [item.id, notificationFingerprint(item)]))
      if (observedNotifications.current) {
        const changed = presentedItems.filter((item) => item.readAt == null && observedNotifications.current?.get(item.id) !== notificationFingerprint(item))
        const shouldAlert = changed.filter((item) => !(nextPreferences.quietProgress && item.kind === 'progress'))
        shouldAlert.slice(0, 3).forEach((item) => enqueueToast(item))
        if (shouldAlert.length && nextPreferences.sound) playNotificationSound(normalizeNotificationSoundId(nextPreferences.soundId))
        if (shouldAlert.length && nextPreferences.systemPopup && typeof Notification !== 'undefined' && Notification.permission === 'granted') {
          const first = shouldAlert[0]
          new Notification(first.title, { body: shouldAlert.length > 1 ? `${first.body}（另有 ${shouldAlert.length - 1} 条更新）` : first.body, tag: first.mergeKey })
        }
        for (const [id, fingerprint] of nextObserved) observedNotifications.current.set(id, fingerprint)
        while (observedNotifications.current.size > 500) {
          const oldestId = observedNotifications.current.keys().next().value
          if (!oldestId) break
          observedNotifications.current.delete(oldestId)
        }
      } else {
        observedNotifications.current = nextObserved
      }
      setItems(presentedItems)
      setPreferences(nextPreferences)
      setError(null)
    } catch (cause) {
      if (generation === loadGeneration.current) setError(desktopErrorDetails(cause).message)
    } finally {
      if (generation === loadGeneration.current) setLoading(false)
    }
  }, [activeConversationId, activeEntityId, activeView, enqueueToast, unreadOnly])

  useEffect(() => () => { loadGeneration.current += 1 }, [load])

  useEffect(() => {
    const refresh = () => void load(true)
    window.addEventListener(FOX_NOTIFICATIONS_CHANGED_EVENT, refresh)
    return () => window.removeEventListener(FOX_NOTIFICATIONS_CHANGED_EVENT, refresh)
  }, [load])

  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    void load(true)
    const timer = window.setInterval(() => void load(true), open ? 5000 : 30000)
    return () => window.clearInterval(timer)
  }, [load, open])

  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    let disposed = false
    let refreshTimer: number | null = null
    const scheduleRefresh = () => {
      if (disposed) return
      if (refreshTimer !== null) window.clearTimeout(refreshTimer)
      refreshTimer = window.setTimeout(() => {
        refreshTimer = null
        void load(true)
      }, 80)
    }
    const visibilityRefresh = () => {
      if (document.visibilityState === 'visible') scheduleRefresh()
    }
    window.addEventListener('focus', scheduleRefresh)
    document.addEventListener('visibilitychange', visibilityRefresh)
    const unsubscribe = subscribeNotificationRefresh(desktopClient, scheduleRefresh,
      () => setError('实时通知连接失败，将继续定时同步；也可以手动刷新通知。'))
    return () => {
      disposed = true
      if (refreshTimer !== null) window.clearTimeout(refreshTimer)
      window.removeEventListener('focus', scheduleRefresh)
      document.removeEventListener('visibilitychange', visibilityRefresh)
      unsubscribe()
    }
  }, [load])

  const unreadCount = items.filter((item) => item.readAt == null).length
  const hasReadNotifications = items.some((item) => item.readAt != null)
  const priorityItems = useMemo(() => items.filter((item) => manuallyPinnedIds.has(item.id) || isAutomaticPriorityNotification(item)), [items, manuallyPinnedIds])
  const datedItems = useMemo(() => items.filter((item) => !manuallyPinnedIds.has(item.id) && !isAutomaticPriorityNotification(item)), [items, manuallyPinnedIds])
  const notificationGroups = useMemo(() => datedItems.reduce<Array<{ key: string; label: string; items: AppNotificationRecord[] }>>((groups, item) => {
    const day = notificationDay(item)
    const current = groups.at(-1)
    if (current?.key === day.key) current.items.push(item)
    else groups.push({ ...day, items: [item] })
    return groups
  }, []), [datedItems])
  const toggleManualPin = useCallback((id: string) => {
    setManuallyPinnedIds((current) => {
      const next = new Set(current)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      window.localStorage.setItem(NOTIFICATION_PIN_STORAGE_KEY, JSON.stringify([...next]))
      return next
    })
  }, [])
  const inspectItem = async (item: AppNotificationRecord) => {
    if (item.readAt == null) await desktopClient.setAppNotificationRead(item.id, true)
    setExpandedId((current) => current === item.id ? null : item.id)
    await load(true)
  }
  const openItem = async (item: AppNotificationRecord) => {
    if (item.readAt == null) await desktopClient.setAppNotificationRead(item.id, true)
    if (item.workspaceView) {
      onNavigate(item.workspaceView as WorkspaceView, item.entityId ?? undefined)
      onOpenChange(false)
    }
    await load(true)
  }
  const openNotificationFile = async (item: AppNotificationRecord, reveal: boolean) => {
    const path = typeof item.action.path === 'string' ? item.action.path : null
    if (!path) return
    try {
      await desktopClient.openDownloadedFile(path, reveal)
    } catch (cause) {
      toast.error(reveal ? '无法打开所在文件夹' : '无法打开文件', { description: desktopErrorDetails(cause).message })
    }
  }
  const markAllRead = async () => {
    try {
      await desktopClient.markAllAppNotificationsRead()
      clearToastQueue()
      await load(true)
    } catch (cause) {
      toast.error('全部标记已读失败', { description: desktopErrorDetails(cause).message })
    }
  }
  const clearRead = async () => {
    try {
      await desktopClient.clearReadAppNotifications()
      clearToastQueue()
      setExpandedId(null)
      await load(true)
    } catch (cause) {
      toast.error('清除已读通知失败', { description: desktopErrorDetails(cause).message })
    }
  }
  const renderNotificationItem = (item: AppNotificationRecord) => {
    const expanded = expandedId === item.id
    const manuallyPinned = manuallyPinnedIds.has(item.id)
    const automaticPriority = isAutomaticPriorityNotification(item)
    const downloadedFile = item.action.type === 'downloaded_file' && typeof item.action.path === 'string'
    const canOpenDirectly = downloadedFile && item.action.canOpenDirectly === true
    return <article key={item.id} className={`fox-notification-entry ${expanded ? 'is-expanded' : ''}`}>
      <div className={`fox-notification-item fox-sidebar-tree-row is-${item.kind} ${item.readAt == null ? 'is-unread' : ''} ${expanded ? 'is-active' : ''} ${manuallyPinned ? 'is-manually-pinned' : ''}`}>
        <button type="button" className="fox-notification-item-main" onClick={() => void inspectItem(item)} aria-expanded={expanded}>
          <span className="fox-notification-dot" />
          <span className="fox-notification-copy"><span className="fox-notification-line"><strong>{item.title}</strong><time>{new Date(item.updatedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time></span><small>{item.body}</small>{item.progress != null && item.kind === 'progress' && <i><b style={{ width: `${item.progress}%` }} /></i>}</span>
        </button>
        {!automaticPriority && <button type="button" className="fox-notification-pin" aria-label={manuallyPinned ? `取消置顶 ${item.title}` : `置顶 ${item.title}`} aria-pressed={manuallyPinned} title={manuallyPinned ? '取消置顶' : '置顶'} onClick={() => toggleManualPin(item.id)}>{manuallyPinned ? <PinOff /> : <Pin />}</button>}
      </div>
      {expanded && <div className="fox-notification-details">
        <pre>{item.body}</pre>
        <footer><span>{item.sourceType} · {item.status}</span><div>{canOpenDirectly && <Button variant="outline" size="sm" onClick={() => void openNotificationFile(item, false)}><ExternalLink />打开文件</Button>}{downloadedFile && <Button variant="outline" size="sm" onClick={() => void openNotificationFile(item, true)}><FolderOpen />打开所在文件夹</Button>}{item.workspaceView && <Button variant="outline" size="sm" onClick={() => void openItem(item)}><ExternalLink />打开相关页面</Button>}</div></footer>
      </div>}
    </article>
  }
  return <>
    {showTrigger && <button type="button" className={`fox-notification-trigger ${open ? 'is-active' : ''}`} aria-label={unreadCount ? `${unreadCount} 条未读通知` : '通知中心'} aria-pressed={open} title="通知中心" onClick={() => onOpenChange(!open)}>
        <Bell />{preferences?.badge !== false && unreadCount > 0 && <span aria-hidden="true" />}
      </button>}
    {open && <section className="fox-notification-sidebar fox-sidebar-section" aria-label="通知中心">
      <header className="fox-section-head">
        <span>通知中心</span>
        <span>
          <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="fox-notification-clear-read" aria-label="清除已读通知" disabled={!hasReadNotifications} onClick={() => void clearRead()}><Eraser /></Button></TooltipTrigger><TooltipContent side="right"><p>清除已读</p></TooltipContent></Tooltip>
          <DropdownMenu>
          <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-notification-more" aria-label="管理通知"><MoreHorizontal /></Button></DropdownMenuTrigger>
          <DropdownMenuContent align="end" side="right" sideOffset={6} className="fox-notification-menu">
            <DropdownMenuItem onSelect={() => setUnreadOnly((value) => !value)}>{unreadOnly ? <Bell /> : <CircleDot />}{unreadOnly ? '显示全部通知' : '仅显示未读'}</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => void load()}><RotateCcw />刷新通知</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem disabled={!unreadCount} onSelect={() => void markAllRead()}><Check />全部已读</DropdownMenuItem>
          </DropdownMenuContent>
          </DropdownMenu>
        </span>
      </header>
      <div className="fox-notification-list" aria-live="polite">
        {error && <div className="fox-notification-error" role="alert"><AlertTriangle />{error}</div>}
        {loading && !items.length && <div className="fox-notification-state"><LoaderCircle className="animate-spin" />正在同步任务状态</div>}
        {!loading && !items.length && <div className="fox-notification-state"><Bell />没有通知</div>}
        {items.length > 0 && <section className="fox-notification-day is-priority" aria-labelledby="notification-priority-title">
          <div className="fox-notification-day-title" id="notification-priority-title">置顶</div>
          <div className="fox-notification-day-items">
            {priorityItems.length ? priorityItems.map(renderNotificationItem) : <p className="fox-notification-priority-empty">暂无置顶通知</p>}
          </div>
        </section>}
        {notificationGroups.map((group) => <section className="fox-notification-day" key={group.key} aria-labelledby={`notification-day-${group.key}`}>
          <div className="fox-notification-day-title" id={`notification-day-${group.key}`}>{group.label}</div>
          <div className="fox-notification-day-items">
            {group.items.map(renderNotificationItem)}
          </div>
        </section>)}
      </div>
    </section>}
    <div className="fox-notification-toast-host" aria-live="polite" aria-atomic="false">
      {toastItems.map((item) => <article key={item.instanceId} className={`fox-notification-toast is-${item.notification.kind} ${item.leaving ? 'is-leaving' : ''}`} role="status">
        <span className="fox-notification-toast-icon"><Bell /></span>
        <span className="fox-notification-toast-copy"><strong>{item.notification.title}</strong><small>{item.notification.body}</small></span>
        <button type="button" aria-label="关闭通知" onClick={() => removeToast(item.instanceId)}><X /></button>
        <i className="fox-notification-toast-progress" />
      </article>)}
    </div>
  </>
}

function WindowTitlebar({ leftSidebarCollapsed, onNewChat, onOpenProject, onSettings, onAbout, onToggleSidebar, onZoom }: {
  leftSidebarCollapsed: boolean
  onNewChat: () => void
  onOpenProject: () => void
  onSettings: () => void
  onAbout: () => void
  onToggleSidebar: () => void
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
        <img src={FOX_ASSISTANT_AVATAR} alt="" />
        <strong>Fox</strong>
        <div className="fox-titlebar-sidebar-controls" aria-label="侧边栏控制">
          <button type="button" aria-label={leftSidebarCollapsed ? '展开左侧边栏' : '收起左侧边栏'} title={leftSidebarCollapsed ? '展开左侧边栏' : '收起左侧边栏'} aria-pressed={!leftSidebarCollapsed} onClick={onToggleSidebar}><PanelLeft /></button>
        </div>
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
            <DropdownMenuItem onSelect={onToggleSidebar}><PanelLeft />切换侧边栏<DropdownMenuShortcut>Ctrl B</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => onZoom(0.1)}>放大<DropdownMenuShortcut>Ctrl +</DropdownMenuShortcut></DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onZoom(-0.1)}>缩小<DropdownMenuShortcut>Ctrl -</DropdownMenuShortcut></DropdownMenuItem>
          </WindowMenu>
          <WindowMenu label="帮助">
            <DropdownMenuItem onSelect={onAbout}><Sparkles />关于 Fox</DropdownMenuItem>
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

const MemoizedWindowTitlebar = memo(WindowTitlebar)

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
  onModeChange,
  onNewChat,
  onNewProjectChat,
  onAddProject,
  onCreateLocalKnowledge,
  onOpenConversation,
  onRenameConversation,
  onPinConversation,
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
  const projectSectionRef = useRef<HTMLElement | null>(null)
  const conversationSectionRef = useRef<HTMLElement | null>(null)
  const normalizedSearch = search.trim().toLocaleLowerCase()
  const changeSearchOpen = useCallback((open: boolean) => {
    setSearchOpen(open)
    if (open) {
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
    { section: 'home', label: '基本设置', icon: House },
    { section: 'resources', label: '能力配置', icon: Settings2 },
    { section: 'conversations', label: '对话任务', icon: ClipboardList },
    { section: 'memory', label: '记忆', icon: Brain },
    { section: 'skills', label: '技能', icon: WandSparkles },
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
                    <div className={`fox-project-row ${project.items.some((item) => item.active) ? 'is-active' : ''}`}>
                      <button type="button" className="fox-project-row-main" title={project.root} aria-expanded={open} onClick={() => setProjectOpen((current) => ({ ...current, [project.key]: !open }))}>
                        {open ? <FolderOpen size={15} /> : <Folder size={15} />}
                        <strong>{project.name}</strong>
                      </button>
                      <span className="fox-project-row-actions">
                        <IconButton label={`在 ${project.name} 中新建对话`} className="fox-project-add" onClick={() => onNewProjectChat(project.root)}><Plus size={13} /></IconButton>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label={`更多 ${project.name} 项目操作`}><MoreHorizontal size={13} /></Button></DropdownMenuTrigger>
                          <DropdownMenuContent align="end" side="right" sideOffset={5} className="fox-project-menu">
                            <DropdownMenuItem onSelect={() => onNewProjectChat(project.root)}><MessageSquarePlus />新建对话</DropdownMenuItem>
                            <DropdownMenuItem onSelect={() => onNavigate('settings-projects')}><ShieldCheck />项目与权限</DropdownMenuItem>
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
                    <ContextMenuSeparator />
                    <ContextMenuItem variant="destructive" onSelect={() => onDeleteProject(project)}><Trash2 />删除项目</ContextMenuItem>
                  </ContextMenuContent>
                </ContextMenu>
                {open && <div className="fox-project-children">{project.items.map((item) => (
                  <ContextMenu key={item.id}>
                    <ContextMenuTrigger asChild>
                      <div className={`fox-sidebar-tree-row fox-project-conversation-row ${item.active ? 'is-active' : ''}`}>
                        <button type="button" className="fox-sidebar-tree-main" onClick={() => onOpenConversation(item.id)}><span>{item.title}</span></button>
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
                      <button type="button" className="fox-sidebar-tree-main" onClick={() => onOpenConversation(item.id)}><MessageCircleMore size={14} /><span>{item.title}</span></button>
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
          <KnowledgeResourceExplorer
            knowledgeId={activeEntityId}
            selectedDocumentId={activeDocumentId}
            navigate={onNavigate}
          />
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
                {project.items.map((item) => <button type="button" role="menuitem" key={item.id} className={item.active ? 'is-active' : undefined} onClick={() => onOpenConversation(item.id)} title={item.title}>
                  <MessageCircleMore />
                  <span>{item.title}</span>
                  {item.time && <time>{item.time}</time>}
                </button>)}
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
            {visibleConversations.length ? visibleConversations.map((item) => <button type="button" role="menuitem" key={item.id} className={item.active ? 'is-active' : undefined} onClick={() => onOpenConversation(item.id)} title={item.title}>
              <MessageCircleMore />
              <span>{item.title}</span>
              {item.time && <time>{item.time}</time>}
            </button>) : <p className="fox-sidebar-collapsed-empty">暂无对话</p>}
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
        <DialogContent className="fox-global-search-dialog" showCloseButton={false}>
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

const MemoizedSidebar = memo(Sidebar)

function AssistantProcess({ running = false }: { running?: boolean }) {
  const [open, setOpen] = useState(false)

  return (
    <ChainOfThought open={open} onOpenChange={setOpen} className="fox-chain-of-thought">
      <ChainOfThoughtHeader className="fox-chain-of-thought-header">
        {running ? <RunStatusText text="正在处理" /> : <span>工作过程（7 步）</span>}
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
          <span className="fox-prompt-figure"><img src={FOX_ASSISTANT_AVATAR} alt="" /></span>
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

function RuntimeApprovalPrompt({ approval, onResolve }: { approval: ApprovalRecord; onResolve: (approvalId: string, decision: ApprovalDecision) => void | Promise<boolean> }) {
  const request = approval.request
  const presentation = approvalPresentation(approval)
  const allowedDecisions = allowedApprovalDecisions(request)
  const repairOverride = isRepairOverrideApproval(request)
  const repairDetails = repairOverrideApprovalDetails(request)
  const [submitting, setSubmitting] = useState(false)
  const submittingRef = useRef(false)
  const resolve = async (decision: ApprovalDecision) => {
    if (submittingRef.current) return
    submittingRef.current = true
    setSubmitting(true)
    const resolved = await resolveAllowedApprovalDecision(approval, decision, onResolve)
    if (!resolved) {
      submittingRef.current = false
      setSubmitting(false)
    }
  }
  return (
    <Confirmation approval={{ id: approval.id }} state="approval-requested" className="fox-confirmation fox-runtime-confirmation">
      <ConfirmationRequest>
        <div className="fox-confirmation-body">
          <div className="fox-confirmation-content"><ConfirmationTitle>{presentation.title}</ConfirmationTitle><p><code>{presentation.target}</code></p><small>{presentation.summary}</small></div>
        </div>
        {repairOverride && <div className={`fox-approval-context ${repairDetails ? '' : 'is-invalid'}`}>
          {repairDetails
            ? <><p><strong>为什么还要再修一次：</strong>{repairDetails.rootCause}</p><p><strong>关联的审查问题：</strong>{repairDetails.findingIds.join('、')}</p></>
            : <p><strong>审批详情不完整。</strong>请先拒绝，并让 Fox 带上根因和关联审查问题重新发起。</p>}
        </div>}
        {presentation.command && <div className="fox-approval-command"><Terminal size={13} /><code>{presentation.command}</code></div>}
        {presentation.diff && <pre className="fox-approval-diff" aria-label="拟修改差异"><code>{presentation.diff}</code></pre>}
        {presentation.content !== undefined && <div className="fox-approval-context"><strong>拟写入内容</strong><pre className="fox-approval-diff" aria-label="拟写入内容"><code>{presentation.content || '（空文件）'}</code></pre></div>}
        <ConfirmationActions className="fox-confirmation-actions">
          <ConfirmationAction variant="ghost" disabled={submitting} onClick={() => void resolve('deny')}>拒绝</ConfirmationAction>
          {allowedDecisions.includes('allow_once') && <ConfirmationAction variant="outline" disabled={submitting} onClick={() => void resolve('allow_once')}>只允许这一次</ConfirmationAction>}
          {allowedDecisions.includes('allow_conversation') && <ConfirmationAction disabled={submitting} onClick={() => void resolve('allow_conversation')}>{submitting ? '处理中…' : '本次对话始终允许'}</ConfirmationAction>}
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
          <img className="fox-inline-mascot" src={FOX_ASSISTANT_AVATAR} alt="" />
          <span><strong>操作已取消</strong><small>Fox 没有修改任何本地文件。</small></span>
        </div>
      </ConfirmationRejected>
    </Confirmation>
  )
}

function errorPresentation(error?: string | null, details?: DesktopErrorDetails | null) {
  const detail = error?.trim() ?? ''
  const expertError = expertErrorPresentation(details ?? error)
  if (expertError) {
    return {
      title: expertError.title,
      description: expertError.description,
      detail: '',
      retryable: expertError.retryable,
      retryLabel: expertError.retryLabel,
    }
  }
  if (/Pi Runtime dependencies|Fox Runtime|runtime (?:script|executable)|Node\.js is required/i.test(detail)) {
    return {
      title: 'Fox 本地运行时不可用',
      description: '本地专家运行环境未能启动。请确认 Fox 依赖完整，并在重启应用后重试。',
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

function ErrorPrompt({ onRetry, error, errorDetails }: { onRetry: () => void; error?: string | null; errorDetails?: DesktopErrorDetails | null }) {
  const presentation = errorPresentation(error, errorDetails)
  return (
    <div className="fox-error-prompt" role="alert">
      <span className="fox-prompt-figure"><img src={FOX_ASSISTANT_AVATAR} alt="" /></span>
      <div><strong>{presentation.title}</strong><p>{presentation.description}</p>{presentation.detail && <small>{presentation.detail}</small>}</div>
      {presentation.retryable !== false && <Button size="sm" variant="outline" onClick={onRetry}><RotateCcw size={14} />{presentation.retryLabel ?? '重试'}</Button>}
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
        <span className="fox-prompt-figure"><img src={FOX_ASSISTANT_AVATAR} alt="" /></span>
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
  if (tool.isError) return `执行失败 ${tool.name}${subject ? ` · ${subject}` : ''}`
  const verb = tool.completed ? '已完成' : '正在调用'

  if (tool.name === 'ask_user_question') return tool.awaitingUser ? '等待你的回答' : 'Fox 正在向你提问'

  if (tool.name === 'read') return `${tool.completed ? '已读取' : 'Fox 正在读取'}${path ? ` ${path}` : '文件'}`
  if (tool.name === 'ls') return `${tool.completed ? '已查看' : 'Fox 正在查看'}${path ? ` ${path}` : '文件夹'}`
  if (tool.name === 'find') return `${tool.completed ? '已查找' : 'Fox 正在查找'}${query ? ` ${query}` : '文件'}${path ? ` · ${path}` : ''}`
  if (tool.name === 'grep') return `${tool.completed ? '已搜索' : 'Fox 正在搜索'}${query ? ` ${query}` : '文件内容'}${path ? ` · ${path}` : ''}`
  if (/web|browser|search/i.test(tool.name)) return `${tool.completed ? '已搜索' : 'Fox 正在搜索'}${subject ? ` ${subject}` : '网页'}`
  return `${tool.completed ? verb : `Fox ${verb}`} ${tool.name}${subject ? ` · ${subject}` : ''}`
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
  const reasoning = reasoningGroups.map((group) => group.text.trim()).filter(Boolean).join('\n\n')
  return {
    reasoning,
    tools: [...tools.values()].sort((left, right) => left.seq - right.seq),
    sources: [...sources.values()],
  }
}

function latestRuntimeDiagnostics(events: RunEventRecord[]) {
  const snapshot = [...events].reverse().find((item) => item.eventType === 'run.request_snapshot')?.event
  const phase = [...events].reverse().find((item) => item.eventType === 'run.phase')?.event.phase
  const retries = events.filter((item) => item.eventType === 'run.retrying').length
  const compactions = events.filter((item) => item.eventType === 'context.compaction.completed').length
  const planner = [...events].reverse().find((item) => (
    item.eventType === 'planner.completed' || item.eventType === 'planner.failed'
  ))
  const toolNames = Array.isArray(snapshot?.toolNames)
    ? snapshot.toolNames.filter((name): name is string => typeof name === 'string')
    : []
  return {
    provider: typeof snapshot?.provider === 'string' ? snapshot.provider : '',
    apiType: typeof snapshot?.apiType === 'string' ? snapshot.apiType : '',
    baseUrl: typeof snapshot?.baseUrl === 'string' ? snapshot.baseUrl : '',
    phase: typeof phase === 'string' ? phase : '',
    retries,
    compactions,
    plannerStatus: planner?.eventType === 'planner.completed'
      ? '计划已生成'
      : planner?.eventType === 'planner.failed' ? '计划已回退' : '',
    toolCount: toolNames.length,
  }
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

function RuntimeArtifacts({ artifacts, onOpenArtifact }: { artifacts: ArtifactRecord[]; onOpenArtifact?: (artifact: ArtifactRecord) => void }) {
  if (!artifacts.length) return null
  return <section className="fox-message-file-results" aria-label="本次文件结果">
    <div className="fox-message-file-results-head"><FileEdit size={13} /><strong>本次文件结果</strong><span>{artifacts.length}</span></div>
    <div className="fox-message-artifacts">
      {artifacts.map((artifact) => {
        const isWeb = artifact.mediaType === 'text/html' || /html|web/i.test(artifact.artifactType)
        const ArtifactIcon = isWeb ? Globe2 : artifact.mediaType?.startsWith('image/') ? ImagePlus : FileText
        const changeLabel = artifact.artifactType === 'created_file' ? '新建' : artifact.artifactType === 'modified_file' ? '已修改' : '文件结果'
        return <button type="button" key={artifact.id} className="fox-message-artifact-trigger" onClick={() => onOpenArtifact?.(artifact)}>
          <Artifact className="fox-message-artifact" title={artifact.displayName}>
            <ArtifactHeader className="fox-message-artifact-head">
              <div className="fox-message-artifact-title">
                <span className="fox-message-artifact-icon"><ArtifactIcon size={15} /></span>
                <div><ArtifactTitle>{artifact.displayName}</ArtifactTitle><ArtifactDescription>{changeLabel} · {formatFileSize(artifact.byteSize)}</ArtifactDescription></div>
              </div>
              <ChevronRight size={14} />
            </ArtifactHeader>
          </Artifact>
        </button>
      })}
    </div>
  </section>
}

function RuntimeAssistantMessage({ message, processEvents, running, artifacts, assistantName, modelName, knowledgeBindings, onOpenSource, onOpenArtifact, onFork }: { message: ConversationMessage; processEvents: RunEventRecord[]; running: boolean; artifacts: ArtifactRecord[]; assistantName: string; modelName?: string; knowledgeBindings?: KnowledgeBindingRecord[]; onOpenSource?: (source: RuntimeSource) => void; onOpenArtifact?: (artifact: ArtifactRecord) => void; onFork?: (messageId: string) => void }) {
  const parsed = useMemo(() => splitAssistantContent(message.content ?? ''), [message.content])
  const process = useMemo(() => runtimeProcess(processEvents), [processEvents])
  const completed = !running && ['completed', 'interrupted', 'failed', 'cancelled'].includes(message.status)
  const replyTime = new Date(message.updatedAt || message.createdAt).toLocaleString([], { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
  const [feedback, setFeedback] = useState<'positive' | 'negative' | null>(null)
  const [feedbackOpen, setFeedbackOpen] = useState(false)
  const [feedbackCategory, setFeedbackCategory] = useState<'irrelevant' | 'code_error' | 'misunderstanding' | 'other'>('irrelevant')
  const [feedbackComment, setFeedbackComment] = useState('')
  const [feedbackSaving, setFeedbackSaving] = useState(false)
  const submitPositiveFeedback = async () => {
    setFeedbackSaving(true)
    try {
      await desktopClient.saveMessageFeedback(message.id, 'positive')
      setFeedback('positive')
      toast.success('感谢反馈')
    } catch (cause) {
      toast.error('反馈保存失败', { description: desktopErrorDetails(cause).message })
    } finally {
      setFeedbackSaving(false)
    }
  }
  const submitNegativeFeedback = async () => {
    setFeedbackSaving(true)
    try {
      await desktopClient.saveMessageFeedback(message.id, 'negative', feedbackCategory, feedbackComment.trim() || undefined)
      setFeedback('negative')
      setFeedbackOpen(false)
      toast.success('反馈已保存')
    } catch (cause) {
      toast.error('反馈保存失败', { description: desktopErrorDetails(cause).message })
    } finally {
      setFeedbackSaving(false)
    }
  }

  return (
    <div className="fox-turn-anchor">
      <Message from="assistant" className="fox-message fox-assistant-message">
        <FoxAssistantAvatar />
        <MessageContent className="fox-assistant-content">
          <RuntimeProcess events={processEvents} process={process} running={running} answerStarted={Boolean(parsed.answer)} />
          {parsed.answer && <div className="fox-answer-body"><MarkdownResponse className="fox-answer-response">{parsed.answer}</MarkdownResponse></div>}
          {!running && <RuntimeArtifacts artifacts={artifacts} onOpenArtifact={onOpenArtifact} />}
          <RuntimeSources sources={process.sources} knowledgeBindings={knowledgeBindings} onOpenSource={onOpenSource} />
        </MessageContent>
        <div className="fox-message-footer">
          <MessageActions className="fox-message-actions">
            <MessageAction tooltip="复制" onClick={() => parsed.answer ? void copyTextWithFeedback(parsed.answer, '已复制回复') : toast('暂无可复制内容')}><Copy size={14} /></MessageAction>
            <MessageAction tooltip={running ? '运行完成后可创建分支' : '从此消息创建分支'} disabled={running} onClick={() => onFork ? onFork(message.id) : requestConversationFork(message.id)}><GitFork size={14} /></MessageAction>
            <MessageAction tooltip="这条回答有帮助" className={feedback === 'positive' ? 'is-feedback-selected' : ''} aria-pressed={feedback === 'positive'} disabled={running || feedbackSaving} onClick={() => void submitPositiveFeedback()}><ThumbsUp size={14} /></MessageAction>
            <MessageAction tooltip="这条回答需要改进" className={feedback === 'negative' ? 'is-feedback-selected' : ''} aria-pressed={feedback === 'negative'} disabled={running || feedbackSaving} onClick={() => setFeedbackOpen(true)}><ThumbsDown size={14} /></MessageAction>
          </MessageActions>
          <div className="fox-message-meta"><span>{assistantName}</span>{modelName && modelName !== assistantName && <span>{modelName}</span>}<span>{replyTime}</span>{!completed && <span className="fox-message-live-meta">处理中</span>}{message.status === 'interrupted' && <span>已中断 · 内容已保存</span>}</div>
        </div>
      </Message>
      <Dialog open={feedbackOpen} onOpenChange={setFeedbackOpen}>
        <DialogContent className="fox-feedback-dialog">
          <DialogHeader><DialogTitle>这条回答哪里需要改进？</DialogTitle><DialogDescription>只保存分类、备注和运行标识，不保存回答正文、文件内容或推理过程。</DialogDescription></DialogHeader>
          <div className="fox-feedback-categories" role="radiogroup" aria-label="反馈分类">
            {([
              ['irrelevant', '答非所问'],
              ['code_error', '代码错误'],
              ['misunderstanding', '理解偏差'],
              ['other', '其他'],
            ] as const).map(([value, label]) => <button type="button" role="radio" aria-checked={feedbackCategory === value} className={feedbackCategory === value ? 'is-selected' : ''} key={value} onClick={() => setFeedbackCategory(value)}>{label}</button>)}
          </div>
          <label className="fox-feedback-comment"><span>补充说明 <small>可选</small></span><Textarea value={feedbackComment} maxLength={1000} placeholder="告诉我们具体问题，最多 1000 字" onChange={(event) => setFeedbackComment(event.target.value)} /><small>{feedbackComment.length} / 1000</small></label>
          <DialogFooter><Button variant="outline" onClick={() => setFeedbackOpen(false)}>取消</Button><Button disabled={feedbackSaving} onClick={() => void submitNegativeFeedback()}>{feedbackSaving && <LoaderCircle className="animate-spin" />}提交反馈</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

const MemoizedRuntimeAssistantMessage = memo(RuntimeAssistantMessage, (previous, next) => (
  previous.message.id === next.message.id
  && previous.message.content === next.message.content
  && previous.message.status === next.message.status
  && previous.message.runId === next.message.runId
  && previous.message.createdAt === next.message.createdAt
  && previous.message.updatedAt === next.message.updatedAt
  && previous.processEvents === next.processEvents
  && previous.running === next.running
  && previous.artifacts === next.artifacts
  && previous.assistantName === next.assistantName
  && previous.modelName === next.modelName
  && previous.knowledgeBindings === next.knowledgeBindings
  && previous.onOpenSource === next.onOpenSource
  && previous.onOpenArtifact === next.onOpenArtifact
  && previous.onFork === next.onFork
))

function RuntimeReasoningItem({ detail, running = false }: { detail: string; running?: boolean }) {
  return (
    <ChainOfThoughtStep icon={Brain} label={<RuntimeStepDetail label="深度思考" active={running}><ReasoningText text={detail} /></RuntimeStepDetail>} status={running ? 'active' : 'complete'} />
  )
}

function LiveReasoning({ detail, active }: { detail: string; active: boolean }) {
  const tail = detail.slice(-800)
  return (
    <div className="fox-live-reasoning" aria-label="深度思考">
      <div className="fox-live-reasoning-head"><Brain size={14} /><RunStatusText text="深度思考" active={active} /><small>最近进度 · 展开查看全部</small></div>
      <div className="fox-live-reasoning-scroll">{tail}</div>
    </div>
  )
}

function RuntimeToolItem({ tool, active }: { tool: RuntimeToolStep; active: boolean }) {
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
      <RuntimeStepDetail label={toolActivity(tool)} active={active}>
        <AIToolInput className="fox-runtime-tool-detail" input={tool.input as never} />
        {tool.completed && <AIToolOutput className="fox-runtime-tool-detail" output={tool.output as never} errorText={tool.isError ? '工具执行失败' : undefined} />}
      </RuntimeStepDetail>
    } status={tool.awaitingUser ? 'pending' : active ? 'active' : 'complete'} />
  )
}

function RuntimeStepDetail({ label, active = false, children }: { label: string; active?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(false)
  const [visited, setVisited] = useState(false)
  return (
    <Collapsible open={open} onOpenChange={(next) => { setOpen(next); if (next) setVisited(true) }} className="fox-runtime-step-detail">
      <CollapsibleTrigger className="fox-runtime-step-trigger">
        <RunStatusText text={label} active={active} /><ChevronRight />
      </CollapsibleTrigger>
      {visited && <CollapsibleContent forceMount className="fox-runtime-step-content" aria-hidden={!open} inert={!open}>
        <div className="fox-runtime-detail-clip"><div className="fox-runtime-detail-panel">{children}</div></div>
      </CollapsibleContent>}
    </Collapsible>
  )
}

function RuntimeProcess({ events, process: preparedProcess, running: runtimeRunning, answerStarted = false }: { events: RunEventRecord[]; process?: ReturnType<typeof runtimeProcess>; running: boolean; answerStarted?: boolean }) {
  const [open, setOpen] = useState(false)
  const [visited, setVisited] = useState(false)
  const process = useMemo(() => preparedProcess ?? runtimeProcess(events), [events, preparedProcess])
  const activity = useMemo(() => runtimeProcessActivity(events, runtimeRunning, answerStarted), [events, runtimeRunning, answerStarted])
  const { running, terminalEvent } = activity
  const hasLifecycle = running || Boolean(terminalEvent) || events.some((item) => (
    item.eventType === 'run.started' || item.eventType === 'message.started'
  ))
  const processStepCount = process.tools.length + (process.reasoning ? 1 : 0)
  const lifecycleOnly = processStepCount === 0 && hasLifecycle
  const stepCount = lifecycleOnly ? 1 : processStepCount
  const activeTool = [...process.tools].reverse().find((tool) => activity.activeToolIds.has(tool.id) && !tool.completed && !tool.awaitingUser)
  const showLiveReasoning = !open && running && Boolean(process.reasoning) && !answerStarted
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
      ? 'Fox 正在思考'
      : awaitingUser
        ? '等待你的回答'
      : lifecycleFailed
        ? '工作过程未完成'
        : lifecycleCancelled
          ? '工作过程已取消'
          : '工作过程已完成'

  if (stepCount === 0 && !running) return null
  return (
    <ChainOfThought open={open} onOpenChange={(next) => { setOpen(next); if (next) setVisited(true) }} className="fox-chain-of-thought fox-runtime-process">
      <ChainOfThoughtHeader className="fox-chain-of-thought-header" trailing={<small className="fox-runtime-step-count" aria-label={`${stepCount} 个步骤`}>{stepCount}</small>}>
        <span className={`fox-runtime-process-summary ${running ? 'is-running' : ''}`}>
          <RunStatusText text={processTitle} active={running} />
        </span>
      </ChainOfThoughtHeader>
      {showLiveReasoning && <LiveReasoning detail={process.reasoning} active={activity.reasoning} />}
      {visited && <ChainOfThoughtContent forceMount className="fox-chain-of-thought-content fox-runtime-process-disclosure" aria-hidden={!open} inert={!open}>
        <div className="fox-runtime-process-clip"><div className="fox-runtime-process-scroll">
        {processStepCount > 0 ? <>
          {process.reasoning && <RuntimeReasoningItem detail={process.reasoning} running={activity.reasoning} />}
          {process.tools.map((tool) => <RuntimeToolItem key={tool.id} tool={tool} active={activity.activeToolIds.has(tool.id) && !tool.completed && !tool.awaitingUser} />)}
        </> : <ChainOfThoughtStep
          className={`fox-runtime-empty-step ${running ? 'is-running' : 'is-complete'}`}
          icon={running ? LoaderCircle : awaitingUser ? CircleHelp : lifecycleFailed ? AlertTriangle : Check}
          label={<RunStatusText text={lifecycleLabel} active={running} />}
          status={running ? 'active' : awaitingUser ? 'pending' : 'complete'}
        />}
        </div></div>
      </ChainOfThoughtContent>}
    </ChainOfThought>
  )
}

function splitAssistantContent(content: string) {
  const reasoning: string[] = []
  const answer = normalizeAssistantMarkdown(content.replace(/<((?:mm:)?(?:think|thinking|analysis))>([\s\S]*?)<\/\1>|<(?:(?:mm:)?(?:think|thinking|analysis))>([\s\S]*)$|^[\s\S]*?<\/(?:(?:mm:)?(?:think|thinking|analysis))>\s*/gi, (_match, _tag: string | undefined, closed: string | undefined, open: string | undefined) => {
    const value = (closed ?? open ?? '').trim()
    if (value) reasoning.push(value)
    return ''
  }).replace(/<\/(?:(?:mm:)?(?:think|thinking|analysis))>/gi, '').trim())
  return { answer, reasoning: normalizeAssistantMarkdown(reasoning.join('\n\n')) }
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

function ConversationBottomDock({ plan }: { plan?: GoalProgressData['planRevisions'][number] }) {
  const planStatus = plan?.status === 'proposed'
    ? '待批准'
    : plan?.status === 'approved'
      ? '已批准'
      : plan?.status === 'rejected'
        ? '已退回'
        : '已更新'
  return (
    <div className={`fox-conversation-bottom-dock ${plan ? 'has-plan' : ''}`}>
      <ConversationScrollButton className="fox-scroll-button" aria-label="定位到最新消息" />
      {plan && <div className={`fox-plan-floater is-${plan.status}`} role="status" title={[plan.title, plan.summary].filter(Boolean).join('\n')}>
        <ListTodo size={12} />
        <strong>{plan.title || `计划 v${plan.revision}`}</strong>
        <span>v{plan.revision} · {planStatus}</span>
      </div>}
    </div>
  )
}

// Exported for the profile-build performance harness (synthetic long-conversation
// fixture). Renders the real streaming message path without any Tauri dependency.
export function RuntimeTimeline({ messages, attachments, artifacts, expertBindings = [], agents = [], pendingMessage, events, activeRunId, activeRunModel, runtimeRunning, state, streamingText, runtimeError, runtimeErrorDetails, assistantName = 'Fox 默认助手', planRevision, hasEarlierMessages, loadingEarlierMessages, onLoadEarlierMessages, onRetry, onRerun, onFork, onOpenSource, onOpenArtifact, onViewExpert }: { messages: ConversationMessage[]; attachments: AttachmentRecord[]; artifacts: ArtifactRecord[]; expertBindings?: ExpertBindingView[]; agents?: AgentRecord[]; pendingMessage?: ConversationMessage | null; events: RunEventRecord[]; activeRunId?: string; activeRunModel?: string; runtimeRunning: boolean; state: ChatState; streamingText: string; runtimeError?: string | null; runtimeErrorDetails?: DesktopErrorDetails | null; assistantName?: string; planRevision?: GoalProgressData['planRevisions'][number]; hasEarlierMessages?: boolean; loadingEarlierMessages?: boolean; onLoadEarlierMessages?: () => void; onRetry: () => void; onRerun: (messageId: string, prompt: string) => Promise<boolean>; onFork?: (messageId: string) => void; onOpenSource?: (source: RuntimeSource) => void; onOpenArtifact?: (artifact: ArtifactRecord) => void; onViewExpert?: (expertId: string) => void }) {
  const runtimeContext = useContext(TimelineRuntimeContext)
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null)
  const [editingText, setEditingText] = useState('')
  const [rerunning, setRerunning] = useState(false)
  const eventGroupsRef = useRef(new Map<string, RunEventRecord[]>)
  const artifactGroupsRef = useRef(new Map<string, ArtifactRecord[]>)
  const syntheticReasoningRef = useRef(new Map<string, RunEventRecord[]>)
  const parsedContentCacheRef = useRef(new Map<string, ReturnType<typeof splitAssistantContent>>())
  const eventsByRunId = useMemo(() => {
    const next = groupRuntimeRecords(events, eventGroupsRef.current)
    eventGroupsRef.current = next
    return next
  }, [events])
  const artifactsByRunId = useMemo(() => {
    const next = groupRuntimeRecords(artifacts, artifactGroupsRef.current)
    artifactGroupsRef.current = next
    return next
  }, [artifacts])
  const resolvedAssistantName = runtimeContext.assistantName ?? assistantName
  const resolvedRunModel = runtimeContext.runModel ?? activeRunModel
  const storedMessages = useMemo(() => messages.filter((message) => message.role === 'user' || message.role === 'assistant'), [messages])
  const attachmentsByMessageId = useMemo(() => {
    const grouped = new Map<string, AttachmentRecord[]>()
    for (const attachment of attachments) {
      if (!attachment.messageId) continue
      const bucket = grouped.get(attachment.messageId)
      if (bucket) bucket.push(attachment)
      else grouped.set(attachment.messageId, [attachment])
    }
    return grouped
  }, [attachments])
  const agentsById = useMemo(() => new Map(agents.map((agent) => [agent.id, agent])), [agents])
  const visibleMessages = useMemo(() => {
    const pendingAlreadyStored = Boolean(pendingMessage && storedMessages.some((message) =>
      message.role === 'user' && message.content === pendingMessage.content && message.createdAt >= pendingMessage.createdAt - 3000
    ))
    const visible = pendingMessage && !pendingAlreadyStored ? [...storedMessages, pendingMessage] : [...storedMessages]
    const currentRunAssistant = activeRunId
      ? visible.find((message) => message.role === 'assistant' && message.runId === activeRunId)
      : undefined
    const currentRunHasProcess = Boolean(activeRunId && eventsByRunId.has(activeRunId))
    if (activeRunId && !currentRunAssistant && (runtimeRunning || Boolean(streamingText) || currentRunHasProcess)) {
      const latestOrdinal = visible.reduce((highest, message) => Math.max(highest, message.ordinal), 0)
      const now = Date.now()
      visible.push({
        id: `assistant-${activeRunId}`,
        conversationId: visible.at(-1)?.conversationId ?? 'pending',
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
    return visible
  }, [activeRunId, eventsByRunId, pendingMessage, runtimeRunning, storedMessages, streamingText])
  const streamTargetId = useMemo(() => latestRunAssistantId(visibleMessages, activeRunId), [visibleMessages, activeRunId])
  const groupedMessages = useMemo(() => groupAssistantContinuations(visibleMessages, streamTargetId, streamingText), [visibleMessages, streamTargetId, streamingText])
  const timelineEntries = useMemo(() => mergeExpertBindingsIntoTimeline(groupedMessages, expertBindings), [expertBindings, groupedMessages])
  const latestUserMessageId = useMemo(() => {
    for (let index = visibleMessages.length - 1; index >= 0; index -= 1) {
      if (visibleMessages[index].role === 'user') return visibleMessages[index].id
    }
    return undefined
  }, [visibleMessages])
  useEffect(() => {
    if (editingMessageId && editingMessageId !== latestUserMessageId) {
      setEditingMessageId(null)
      setEditingText('')
    }
  }, [editingMessageId, latestUserMessageId])
  const beginEditing = (message: ConversationMessage) => {
    if (runtimeRunning) return
    setEditingMessageId(message.id)
    setEditingText(message.content)
  }
  const cancelEditing = () => {
    if (rerunning) return
    setEditingMessageId(null)
    setEditingText('')
  }
  const rerunEditedMessage = async () => {
    const prompt = editingText.trim()
    if (!prompt) {
      toast.error('消息内容不能为空')
      return
    }
    if (runtimeRunning || rerunning || !editingMessageId) return
    setRerunning(true)
    try {
      const started = await onRerun(editingMessageId, prompt)
      if (started) {
        setEditingMessageId(null)
        setEditingText('')
      }
    } finally {
      setRerunning(false)
    }
  }
  const currentAssistantActivity = useMemo(() => activeRunId
    ? visibleMessages.find((message) => message.role === 'assistant' && message.runId === activeRunId)
    : undefined, [activeRunId, visibleMessages])
  const currentRunLastSeq = useMemo(() => activeRunId
    ? (eventsByRunId.get(activeRunId) ?? EMPTY_RUNTIME_EVENTS).reduce((highest, item) => Math.max(highest, item.seq), 0)
    : 0, [activeRunId, eventsByRunId])
  const autoScrollKey = runtimeRunning
    ? [latestUserMessageId, activeRunId, currentRunLastSeq > 0 || currentAssistantActivity ? 'active' : 'waiting'].join(':')
    : latestUserMessageId
  return (
    <Conversation className="fox-conversation">
      <ConversationAutoScroll scrollKey={autoScrollKey} />
      <ConversationViewportState />
      <ConversationTurnRail messages={groupedMessages} artifacts={artifacts} />
      <ConversationContent className="fox-conversation-content" scrollClassName="fox-conversation-scroll">
        {hasEarlierMessages && <div className="fox-history-loader"><Button variant="ghost" size="sm" disabled={loadingEarlierMessages} onClick={onLoadEarlierMessages}>{loadingEarlierMessages && <LoaderCircle className="animate-spin" />}加载更早消息</Button></div>}
        <div className="fox-thread-date"><span>今天</span></div>
        {timelineEntries.map((entry) => {
          if (entry.kind === 'expert') {
            const expert = agentsById.get(entry.binding.expertId)
            return <ExpertActivationCard key={`expert-binding-${entry.binding.id}`} binding={entry.binding} expert={expert} onView={(expertId) => onViewExpert?.(expertId)} />
          }
          const message = entry.message
          if (message.role === 'user') {
            const messageAttachments = attachmentsByMessageId.get(message.id) ?? EMPTY_CONVERSATION_ATTACHMENTS
            const latest = message.id === latestUserMessageId
            const editing = latest && editingMessageId === message.id
            return <div id={`fox-turn-${message.id}`} key={message.id} className="fox-turn-anchor" data-turn-message-id={message.id}>
              <Message from="user" className={`fox-message fox-user-message ${editing ? 'is-editing' : ''}`}>
                {runtimeContext.userProfile && <UserMessageAvatar profile={runtimeContext.userProfile} />}
                {editing ? <MessageContent className="fox-user-edit">
                  <textarea
                    autoFocus
                    aria-label="编辑最新消息"
                    value={editingText}
                    onChange={(event) => setEditingText(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === 'Escape') cancelEditing()
                      if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
                        event.preventDefault()
                        void rerunEditedMessage()
                      }
                    }}
                  />
                  <div className="fox-user-edit-actions">
                    <Button className="fox-user-edit-cancel" variant="ghost" size="sm" disabled={rerunning} onClick={cancelEditing}>取消</Button>
                    <Button className="fox-user-edit-send" size="sm" disabled={rerunning || !editingText.trim()} onClick={() => void rerunEditedMessage()}>
                      {rerunning ? <LoaderCircle className="animate-spin" size={13} /> : <ArrowUp size={13} />}
                      发送
                    </Button>
                  </div>
                </MessageContent> : <UserMessageBubble>{message.content}</UserMessageBubble>}
                <SentMessageAttachments attachments={messageAttachments} />
                {!editing && <div className="fox-user-meta">
                  <span>Fox · {message.status === 'completed' ? '已发送' : message.status === 'sending' ? '发送中' : message.status}</span>
                  <span className={`fox-user-actions ${latest ? 'is-latest' : ''}`}>
                    <MessageAction tooltip="复制" onClick={() => void copyTextWithFeedback(message.content, '已复制消息')}><Copy size={13} /></MessageAction>
                    <MessageAction tooltip={runtimeRunning ? '运行完成后可创建分支' : '从此消息创建分支'} disabled={runtimeRunning} onClick={() => onFork ? onFork(message.id) : requestConversationFork(message.id)}><GitFork size={13} /></MessageAction>
                    {latest && <MessageAction tooltip={runtimeRunning ? '运行完成后可编辑' : '编辑并重新发送'} disabled={runtimeRunning} onClick={() => beginEditing(message)}><FileEdit size={13} /></MessageAction>}
                  </span>
                </div>}
              </Message>
            </div>
          }
          const isCurrentRun = Boolean(activeRunId && message.runId === activeRunId)
          const rawContent = message.content
          const parsedCacheKey = `${message.id}:${rawContent}`
          let parsed = parsedContentCacheRef.current.get(parsedCacheKey)
          if (!parsed) {
            parsed = splitAssistantContent(rawContent)
            if (parsedContentCacheRef.current.size >= 256) {
              const oldestKey = parsedContentCacheRef.current.keys().next().value
              if (typeof oldestKey === 'string') parsedContentCacheRef.current.delete(oldestKey)
            }
            parsedContentCacheRef.current.set(parsedCacheKey, parsed)
          }
          const storedProcessEvents = message.runId ? eventsByRunId.get(message.runId) ?? EMPTY_RUNTIME_EVENTS : EMPTY_RUNTIME_EVENTS
          const hasStoredReasoning = storedProcessEvents.some((item) => item.eventType === 'reasoning.delta')
          let processEvents = storedProcessEvents
          if (parsed.reasoning && !hasStoredReasoning) {
            const cacheKey = `${message.id}:${parsed.reasoning}`
            const cached = syntheticReasoningRef.current.get(cacheKey)
            if (cached) {
              processEvents = cached
            } else {
              processEvents = [...storedProcessEvents, {
                runId: message.runId ?? `history-${message.id}`,
                seq: storedProcessEvents.reduce((highest, item) => Math.max(highest, item.seq), 0) + 1,
                eventType: 'reasoning.delta',
                event: { type: 'reasoning.delta', delta: parsed.reasoning, source: 'yuxi-history' },
                createdAt: message.updatedAt,
              }]
              syntheticReasoningRef.current.set(cacheKey, processEvents)
            }
          }
          const running = message.id === streamTargetId && runtimeRunning
          const messageArtifacts = message.runId ? artifactsByRunId.get(message.runId) ?? EMPTY_RUNTIME_ARTIFACTS : EMPTY_RUNTIME_ARTIFACTS
          const eventModel = processEvents.find((item) => item.eventType === 'run.started' && typeof item.event.model === 'string')?.event.model as string | undefined
          const displayMessage = rawContent === message.content ? message : { ...message, content: rawContent }
          return <div id={`fox-turn-${message.id}`} key={message.timelineKey} className="fox-turn-anchor" data-turn-message-id={message.id}><MemoizedRuntimeAssistantMessage message={displayMessage} processEvents={processEvents} running={running} artifacts={messageArtifacts} assistantName={resolvedAssistantName} modelName={eventModel ?? (isCurrentRun ? resolvedRunModel : undefined)} knowledgeBindings={runtimeContext.knowledgeBindings} onOpenSource={onOpenSource ?? runtimeContext.onOpenSource} onOpenArtifact={onOpenArtifact} onFork={onFork} /></div>
        })}
        {state === 'error' && <div className="fox-turn-anchor"><Message from="assistant" className="fox-message fox-assistant-message"><MessageContent className="fox-assistant-content"><ErrorPrompt onRetry={onRetry} error={runtimeError} errorDetails={runtimeErrorDetails} /></MessageContent></Message></div>}
        {runtimeContext.recoveryPanel}
      </ConversationContent>
      <ConversationBottomDock plan={planRevision} />
    </Conversation>
  )
}

function Timeline({ empty, state, prompt, openingSuggestions, runtimeMessages, runtimeAttachments, runtimeArtifacts, expertBindings, agents, pendingMessage, runtimeEvents, runtimeRunId, runtimeRunModel, runtimeRunning = false, runtimeReply, runtimeError, runtimeErrorDetails, assistantName, planRevisions, hasEarlierMessages, loadingEarlierMessages, onLoadEarlierMessages, onApprove, onDeny, onRetry, onRerun, onFork, onAnswer, onStart, onOpenSource, onOpenArtifact, onViewExpert }: { empty: boolean; state: ChatState; prompt: string; openingSuggestions?: string[]; runtimeMessages?: ConversationMessage[]; runtimeAttachments?: AttachmentRecord[]; runtimeArtifacts?: ArtifactRecord[]; expertBindings?: ExpertBindingView[]; agents?: AgentRecord[]; pendingMessage?: ConversationMessage | null; runtimeEvents?: RunEventRecord[]; runtimeRunId?: string; runtimeRunModel?: string; runtimeRunning?: boolean; runtimeReply?: string; runtimeError?: string | null; runtimeErrorDetails?: DesktopErrorDetails | null; assistantName?: string; planRevisions?: GoalProgressData['planRevisions']; hasEarlierMessages?: boolean; loadingEarlierMessages?: boolean; onLoadEarlierMessages?: () => void; onApprove: () => void; onDeny: () => void; onRetry: () => void; onRerun: (messageId: string, prompt: string) => Promise<boolean>; onFork?: (messageId: string) => void; onAnswer: (answer: string) => void; onStart: (suggestion: string) => void; onOpenSource?: (source: RuntimeSource) => void; onOpenArtifact?: (artifact: ArtifactRecord) => void; onViewExpert?: (expertId: string) => void }) {
  const running = state === 'running'
  const latestPlanRevision = planRevisions?.reduce((latest, plan) => !latest || plan.revision > latest.revision ? plan : latest, undefined as GoalProgressData['planRevisions'][number] | undefined)
  const starterSuggestions = openingSuggestions?.map((item) => item.trim()).filter(Boolean).slice(0, 6)
  const visibleSuggestions = starterSuggestions?.length ? starterSuggestions : [
    '整理这个项目中的文档',
    '从知识库查找相关资料',
    '读取一个文件并帮我分析',
    '帮我规划并完成一个任务',
  ]

  if (empty) {
    return (
        <Conversation className="fox-conversation">
        <ConversationViewportState />
        <ConversationEmptyState className="fox-empty-conversation">
          <div className="fox-empty-heading">
            <NewConversationMascot />
            <div className="fox-empty-heading-copy"><h2>今天想一起做点什么？</h2><p>Fox 可以和你对话、处理本地文件，也可以从知识库中检索资料。</p></div>
          </div>
          <Suggestions className="fox-starter-suggestions">
            {visibleSuggestions.map((suggestion) => <Suggestion key={suggestion} suggestion={suggestion} onClick={onStart} />)}
          </Suggestions>
        </ConversationEmptyState>
      </Conversation>
    )
  }
  if (runtimeMessages) {
    return <RuntimeTimeline messages={runtimeMessages} attachments={runtimeAttachments ?? []} artifacts={runtimeArtifacts ?? []} expertBindings={expertBindings} agents={agents} pendingMessage={pendingMessage} events={runtimeEvents ?? []} activeRunId={runtimeRunId} activeRunModel={runtimeRunModel} runtimeRunning={runtimeRunning} state={state} streamingText={runtimeReply ?? ''} runtimeError={runtimeError} runtimeErrorDetails={runtimeErrorDetails} assistantName={assistantName} planRevision={latestPlanRevision} hasEarlierMessages={hasEarlierMessages} loadingEarlierMessages={loadingEarlierMessages} onLoadEarlierMessages={onLoadEarlierMessages} onRetry={onRetry} onRerun={onRerun} onFork={onFork} onOpenSource={onOpenSource} onOpenArtifact={onOpenArtifact} onViewExpert={onViewExpert} />
  }
  return (
    <Conversation className="fox-conversation">
      <ConversationViewportState />
      <ConversationContent className="fox-conversation-content" scrollClassName="fox-conversation-scroll">
        <div className="fox-thread-date"><span>今天</span></div>
        <div className="fox-turn-anchor">
        <Message from="user" className="fox-message fox-user-message">
          <UserMessageBubble>{prompt}</UserMessageBubble>
          <div className="fox-user-meta"><span>DeepSeek V3.2 · 刚刚</span><span><MessageAction tooltip="复制"><Copy size={13} /></MessageAction><MessageAction tooltip="编辑"><FileEdit size={13} /></MessageAction></span></div>
        </Message>
        </div>
        <div className="fox-turn-anchor">
        <Message from="assistant" className="fox-message fox-assistant-message">
          <MessageContent className="fox-assistant-content">
            {(state === 'running' || state === 'complete') && <AssistantProcess running={running} />}
            {state === 'question' ? null : state === 'error' ? <ErrorPrompt onRetry={onRetry} /> : state === 'denied' ? <DeniedPrompt /> : state === 'approval' ? null : running ? (
              <div className="fox-live-response"><FoxAssistantAvatar /><RunStatusText text="正在连接知识库并整理结果" /><i><b /><b /><b /></i></div>
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
          </MessageActions>}
        </Message>
        </div>
      </ConversationContent>
      <ConversationBottomDock plan={latestPlanRevision} />
    </Conversation>
  )
}

const MemoizedTimeline = memo(Timeline)

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

function ComposerAttachmentPreview({ projectRoot, knowledgeChips = [], onProject, onKnowledge }: { projectRoot?: string | null; knowledgeChips?: Array<{ key: string; name: string }>; onProject?: () => void; onKnowledge?: () => void }) {
  const attachments = usePromptInputAttachments()
  const items: ReactNode[] = []
  if (projectRoot) items.push(<button type="button" key="project" className="is-authorized" onClick={onProject} title={projectRoot}><FolderOpen size={13} /><span>{projectRoot.split(/[\\/]/).filter(Boolean).at(-1)}</span><Check size={12} /></button>)
  knowledgeChips.forEach((chip) => {
    const label = chip.name
    items.push(<button type="button" key={chip.key} className="is-authorized is-knowledge" onClick={onKnowledge} title={label}><Library size={13} /><span>{label}</span><Check size={12} /></button>)
  })
  attachments.files.forEach((file) => items.push(<Attachment key={file.id} data={file} onRemove={() => attachments.remove(file.id)}><AttachmentPreview /><AttachmentInfo /><AttachmentRemove label="移除附件" /></Attachment>))
  if (!items.length) return null
  const labels = [...(projectRoot ? [projectRoot] : []), ...knowledgeChips.map((chip) => chip.name), ...attachments.files.map((file) => file.filename || '附件')]
  return (
    <Attachments variant="inline" className="fox-composer-attachments fox-composer-context fox-composer-selection-row">
      {items.slice(0, 3)}
      {items.length > 3 && <Popover><Tooltip><TooltipTrigger asChild><PopoverTrigger asChild><button type="button" className="fox-composer-overflow" aria-label={`查看其余 ${items.length - 3} 项`}>…</button></PopoverTrigger></TooltipTrigger><TooltipContent className="max-h-64 overflow-auto">{labels.slice(3).map((label, index) => <p key={index}>{label}</p>)}</TooltipContent></Tooltip><PopoverContent className="fox-composer-overflow-list" align="start">{items.slice(3)}</PopoverContent></Popover>}
    </Attachments>
  )
}

function ComposerAttachmentCommandBridge({ openRef }: { openRef: { current: () => void } }) {
  const attachments = usePromptInputAttachments()

  useEffect(() => {
    openRef.current = attachments.openFileDialog
    return () => {
      openRef.current = () => undefined
    }
  }, [attachments.openFileDialog, openRef])

  return null
}

const slashCommands = [
  { id: '目标', action: 'mode', modeLabel: '目标', title: '创建目标', detail: '显式进入需要持续跟踪的工作模式', icon: <Target size={15} /> },
  { id: '文件', action: 'attachments', modeLabel: '文件', title: '添加文件', detail: '打开文件选择器并加入当前消息', icon: <FileText size={15} /> },
  { id: '知识库', action: 'knowledge', modeLabel: '知识库', title: '选择知识库', detail: '从远程或本地知识库中直接选择', icon: <Library size={15} /> },
  { id: '计划', action: 'mode', modeLabel: '计划', title: '规划任务', detail: '先生成执行计划，再开始处理', icon: <ListTodo size={15} /> },
  { id: '总结', action: 'mode', modeLabel: '总结', title: '总结对话', detail: '压缩当前会话并保留关键上下文', icon: <Sparkles size={15} /> }
]

type SlashCommand = typeof slashCommands[number]

function slashCommandRuntimeText(command: SlashCommand, prompt: string) {
  switch (command.id) {
    case '目标': return `/目标 ${prompt}`
    case '计划': return `/计划 ${prompt}`
    case '总结': return `请结合当前对话完成以下总结要求：\n${prompt}`
    default: return prompt
  }
}

type ComposerRuntimeContextValue = {
  knowledgeBindings: KnowledgeBindingRecord[]
  onKnowledge?: () => void
  onOpenSource?: (source: RuntimeSource) => void
  assistantName?: string
  runModel?: string
  questionRequest?: RuntimeQuestionRequest | null
  onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  remoteKnowledgeBases?: KnowledgeBaseRecord[]
  localKnowledgeBases?: LocalKnowledgeBaseDto[]
  knowledgeReferences?: KnowledgeReference[]
  knowledgeReferenceNames?: Record<string, string>
  knowledgeLoading?: boolean
  knowledgeError?: string | null
  onKnowledgeMenuOpen?: () => void
  onKnowledgeToggle?: (reference: KnowledgeReference, name: string) => Promise<boolean>
}

type TimelineRuntimeContextValue = Pick<ComposerRuntimeContextValue,
  'knowledgeBindings' | 'onOpenSource' | 'assistantName' | 'runModel'
> & { recoveryPanel?: ReactNode; userProfile?: UserProfile }

const ComposerRuntimeContext = createContext<ComposerRuntimeContextValue>({ knowledgeBindings: [] })
const TimelineRuntimeContext = createContext<TimelineRuntimeContextValue>({ knowledgeBindings: [] })

const goalStateCopy: Record<ChatState, { label: string; detail: string; time: string }> = {
  complete: { label: '已完成', detail: '确认 Kun 授权范围与 Fox 开发边界', time: '刚刚' },
  running: { label: '进行中', detail: '正在整理当前问题的依据与结论', time: '12 分钟' },
  question: { label: '等待回答', detail: '需要你选择文件处理方式后继续', time: '待你确认' },
  approval: { label: '等待批准', detail: '需要授权后才能写入本地工作区', time: '待你批准' },
  error: { label: '连接错误', detail: '知识库服务暂时无响应，可重试当前步骤', time: '需处理' },
  denied: { label: '已取消', detail: '已取消本次写入，工作区没有变更', time: '刚刚' }
}

function formatGoalElapsed(start: string, end?: string | null) {
  const startTime = Date.parse(start)
  const endTime = end ? Date.parse(end) : Date.now()
  if (!Number.isFinite(startTime) || !Number.isFinite(endTime)) return ''
  const seconds = Math.max(0, Math.floor((endTime - startTime) / 1000))
  if (seconds < 60) return `${seconds}秒`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}分钟`
  const hours = Math.floor(minutes / 60)
  return `${hours}小时 ${String(minutes % 60).padStart(2, '0')}分`
}

function isFileChangeTool(toolName: string) {
  return toolName === 'write_file' || toolName === 'edit_file'
}

function isCommandTool(toolName: string) {
  return /^(run_command|test_run|code_check|format_code|shell|command)$/i.test(toolName)
}

function isChangeOrCommandTool(toolName: string) {
  return isFileChangeTool(toolName) || isCommandTool(toolName)
}

function toolResultDetails(tool: ToolCallRecord) {
  const result = tool.result && typeof tool.result === 'object' && !Array.isArray(tool.result)
    ? tool.result as Record<string, unknown>
    : {}
  const details = result.details && typeof result.details === 'object' && !Array.isArray(result.details)
    ? result.details as Record<string, unknown>
    : result
  return { result, details }
}

function commandOutput(tool: ToolCallRecord) {
  const { result, details } = toolResultDetails(tool)
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
  const stdout = compactValue(details.stdout ?? result.stdout)
  const stderr = compactValue(details.stderr ?? result.stderr)
  const output = [stdout && `stdout:\n${stdout}`, stderr && `stderr:\n${stderr}`].filter(Boolean).join('\n\n')
  const exitCode = details.exitCode ?? result.exitCode
  const cwd = compactValue(details.cwd ?? input.cwd)
  const duration = tool.completedAt !== null
    ? Math.max(0, tool.completedAt - tool.startedAt)
    : null
  return { output, exitCode: typeof exitCode === 'number' ? exitCode : null, cwd, duration }
}

function readStoredSeenChanges() {
  if (typeof window === 'undefined') return {} as Record<string, number>
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(layoutStorage.seenChanges) ?? '{}')
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => Number.isFinite(entry[1]) && entry[1] >= 0))
  } catch {
    return {}
  }
}

function GoalFloater({ chatState, data, onDelete, onRunningChange, onEvidenceClick, onResolveConfirmation, onResolvePlanRevision, onResolveWorkflowGate }: { chatState: ChatState; data?: GoalProgressData | null; onDelete?: (goalId: string) => Promise<boolean> | void; onRunningChange?: (goalId: string, expectedVersion: number, running: boolean) => Promise<boolean> | void; onEvidenceClick?: (evidence: TaskEvidenceRecord) => void; onResolveConfirmation?: (goalId: string, expectedVersion: number, approved: boolean) => Promise<boolean> | void; onResolvePlanRevision?: (planRevisionId: string, decision: 'approved' | 'rejected') => Promise<boolean> | void; onResolveWorkflowGate?: (workflowRunId: string, stageId: string, decision: 'approved' | 'rejected') => Promise<boolean> | void }) {
  const [open, setOpen] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [changingRunning, setChangingRunning] = useState(false)
  const [, setClock] = useState(0)
  useEffect(() => {
    if (!data || !['active', 'blocked'].includes(data.goal.status)) return
    const timer = window.setInterval(() => setClock((value) => value + 1), 1000)
    return () => window.clearInterval(timer)
  }, [data?.goal.id, data?.goal.status])
  const visualState = data?.goal.status === 'active'
    ? 'running'
    : data?.goal.status === 'blocked'
      ? 'paused'
      : data?.goal.status === 'completed'
        ? 'complete'
        : data?.goal.status === 'proposed'
          ? 'question'
          : chatState
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverAnchor asChild>
        <div className={`fox-goal-floater is-${visualState}`}>
          <PopoverTrigger asChild>
            <button type="button" className="fox-goal-trigger" aria-label={open ? '收起当前目标详情' : '打开当前目标详情'}>
              <span className="fox-goal-icon"><Target size={12} /></span>
              <strong>{data?.goal.title || goalStateCopy[chatState].label}</strong>
              {data && <span className="fox-goal-time">{formatGoalElapsed(data.goal.createdAt, ['completed', 'cancelled'].includes(data.goal.status) ? data.goal.completedAt ?? data.goal.updatedAt : null)}</span>}
            </button>
          </PopoverTrigger>
          {data && onRunningChange && ['active', 'blocked'].includes(data.goal.status) && <button type="button" className="fox-goal-running" disabled={changingRunning} aria-label={data.goal.status === 'active' ? '暂停目标并停止当前运行' : '继续目标并唤醒专家'} title={data.goal.status === 'active' ? '暂停目标并停止当前运行' : '继续目标并唤醒专家'} onClick={async () => {
            setChangingRunning(true)
            const changed = await onRunningChange(data.goal.id, data.goal.version, data.goal.status !== 'active')
            setChangingRunning(false)
            if (!changed) toast.error('目标状态更新失败')
            else toast.success(data.goal.status === 'active' ? '目标已暂停，正在停止当前运行' : '目标已继续运行')
          }}>{changingRunning ? <LoaderCircle className="animate-spin" size={11} /> : data.goal.status === 'active' ? <Pause size={11} /> : <Play size={11} />}</button>}
          <button type="button" className="fox-goal-expand" aria-label={open ? '收起当前目标详情' : '打开当前目标详情'} title={open ? '收起' : '展开'} onClick={() => setOpen((value) => !value)}>{open ? <ChevronUp size={12} /> : <ChevronDown size={12} />}</button>
          {data && onDelete && <button type="button" className="fox-goal-delete" disabled={deleting} aria-label="删除目标" title="删除目标" onClick={async () => {
            setDeleting(true)
            const deleted = await onDelete(data.goal.id)
            setDeleting(false)
            if (deleted) {
              setOpen(false)
              toast.success('目标已删除')
            } else {
              toast.error('目标删除失败，请先停止当前生成后重试')
            }
          }}><Trash2 size={11} /></button>}
        </div>
      </PopoverAnchor>
      {data && <PopoverContent align="start" side="top" sideOffset={8} className="fox-goal-popover">
        <GoalProgress data={data} defaultExpanded onEvidenceClick={onEvidenceClick} onResolveConfirmation={onResolveConfirmation} onResolvePlanRevision={onResolvePlanRevision} onResolveWorkflowGate={onResolveWorkflowGate} />
      </PopoverContent>}
    </Popover>
  )
}

function Composer({ resetKey, draft, setDraft, chatState, showGoal = false, centered = false, runtimeControlled = false, runtimeInitializing = false, projectRoot, projectPermissionMode, activeAgent, activeExpert, expertReadOnly = false, expertToolAvailability, agents = [], modelService, runtimeCapabilities, yuxiModels = [], usage, knowledgeBindings = [], questionRequest, goalProgressData, runtimeApprovals = [], onProject, onPermissionModeChange, onKnowledge, onAgentChange, onViewExpert, onChangeExpert, onRemoveExpert, onHeightChange, onPromptCommit, onSubmitPrompt, onSubmitQuestion, onApprove, onDeny, onAnswer, onResolveApproval, onResolveWorkModeConfirmation, onResolvePlanRevision, onResolveWorkflowGate, onDeleteGoal, onGoalRunningChange, onEvidenceClick, onStatusChange, onCancel }: { resetKey: number; draft: string; setDraft: Dispatch<SetStateAction<string>>; chatState: ChatState; showGoal?: boolean; centered?: boolean; runtimeControlled?: boolean; runtimeInitializing?: boolean; projectRoot?: string | null; projectPermissionMode?: ProjectRecord['permissionMode'] | null; activeAgent?: AgentRecord | null; activeExpert?: AgentRecord | null; expertReadOnly?: boolean; expertToolAvailability?: ExpertToolAvailability; agents?: AgentRecord[]; modelService?: ModelServiceRecord | null; runtimeCapabilities?: Record<string, unknown>; yuxiModels?: YuxiModelRecord[]; usage?: ConversationUsage; knowledgeBindings?: KnowledgeBindingRecord[]; questionRequest?: RuntimeQuestionRequest | null; goalProgressData?: GoalProgressData | null; runtimeApprovals?: ApprovalRecord[]; onProject?: () => void; onPermissionModeChange?: (permissionMode: ProjectRecord['permissionMode']) => void | Promise<void>; onKnowledge?: () => void; onAgentChange?: (agentId: string) => void; onViewExpert?: () => void; onChangeExpert?: () => void; onRemoveExpert?: () => void | Promise<void>; onHeightChange?: (height: number) => void; onPromptCommit?: (prompt: string) => void; onSubmitPrompt?: (prompt: string, model?: string, files?: Array<{ filename?: string; mediaType?: string; url?: string }>, runtimeText?: string) => 'complete' | 'question' | 'approval' | 'error' | void | Promise<'complete' | 'question' | 'approval' | 'error' | void>; onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>; onApprove?: () => void; onDeny?: () => void; onAnswer?: (answer: string) => void; onResolveApproval?: (approvalId: string, decision: ApprovalDecision) => void | Promise<boolean>; onResolveWorkModeConfirmation?: (goalId: string, expectedVersion: number, approved: boolean) => Promise<boolean>; onResolvePlanRevision?: (planRevisionId: string, decision: 'approved' | 'rejected') => Promise<boolean>; onResolveWorkflowGate?: (workflowRunId: string, stageId: string, decision: 'approved' | 'rejected') => Promise<boolean>; onDeleteGoal?: (goalId: string) => Promise<boolean>; onGoalRunningChange?: (goalId: string, expectedVersion: number, running: boolean) => Promise<boolean>; onEvidenceClick?: (evidence: TaskEvidenceRecord) => void; onStatusChange?: (status: 'ready' | 'streaming') => void; onCancel?: () => boolean | void | Promise<boolean | void> }) {
  const runtimeContext = useContext(ComposerRuntimeContext)
  const resolveRuntimeApproval = onResolveApproval
  const activeQuestionRequest = questionRequest ?? runtimeContext.questionRequest
  const submitQuestion = onSubmitQuestion ?? runtimeContext.onSubmitQuestion
  const activeKnowledgeBindings = knowledgeBindings.length ? knowledgeBindings : runtimeContext.knowledgeBindings
  const enabledKnowledgeBindings = activeKnowledgeBindings.filter((binding) => binding.enabled !== false)
  const configuredKnowledgeReferences = Array.isArray(runtimeContext.knowledgeReferences)
    ? runtimeContext.knowledgeReferences
    : enabledKnowledgeBindings.map(knowledgeReferenceFromLegacyBinding)
  const selectedKnowledgeReferenceKeys = new Set(configuredKnowledgeReferences.map(knowledgeReferenceKey))
  const knowledgeChips = configuredKnowledgeReferences.map((reference) => ({
    key: knowledgeReferenceKey(reference),
    name: runtimeContext.knowledgeReferenceNames?.[knowledgeReferenceKey(reference)]
      ?? (reference.source === 'local' ? runtimeContext.localKnowledgeBases?.find((item) => item.id === reference.id)?.name
        : runtimeContext.remoteKnowledgeBases?.find((item) => item.id === reference.id)?.name)
      ?? enabledKnowledgeBindings.find((binding) => knowledgeReferenceKey(knowledgeReferenceFromLegacyBinding(binding)) === knowledgeReferenceKey(reference))?.knowledgeBaseName
      ?? reference.id,
  }))
  const selectableAgents = chatSelectableAgents(agents)
  const openKnowledge = onKnowledge ?? runtimeContext.onKnowledge
  const [status, setStatus] = useState<'ready' | 'streaming'>('ready')
  const [focused, setFocused] = useState(false)
  const sendWithModifier = typeof window !== 'undefined' && window.localStorage.getItem('fox.preferences.sendKey') === 'mod-enter'
  const spellcheckEnabled = typeof window === 'undefined' || window.localStorage.getItem('fox.preferences.spellcheck') !== '0'
  const allowYuxiModelOverride = yuxiModelOverrideAllowed(activeAgent)
  const isYuxiAgent = activeAgent?.runtimeType === 'yuxi'
  const supportsImageInput = isYuxiAgent || runtimeCapabilities?.imageInput === true
  const supportsDynamicModelSwitch = isYuxiAgent
    ? allowYuxiModelOverride
    : runtimeCapabilities?.dynamicModelSwitch === true
  const modelOptions = activeAgent?.runtimeType === 'yuxi'
    ? allowYuxiModelOverride ? yuxiModels : []
    : modelService?.modelId
      ? [{ spec: modelService.modelId, modelId: modelService.modelId, displayName: modelService.modelId, providerId: 'fox', providerDisplayName: modelService.name }]
      : []
  const defaultModel = activeAgent?.runtimeType === 'yuxi'
    ? activeAgent.defaultModel
    : modelService?.modelId ?? (activeAgent?.defaultModel !== 'configured-model' ? activeAgent?.defaultModel : undefined) ?? '未配置模型'
  const [model, setModel] = useState(defaultModel)
  const [questionFreeform, setQuestionFreeform] = useState('')
  const [questionSelections, setQuestionSelections] = useState<Record<string, string[]>>({})
  const [questionOtherAnswers, setQuestionOtherAnswers] = useState<Record<string, string>>({})
  const [commandOpen, setCommandOpen] = useState(false)
  const [commandView, setCommandView] = useState<'commands' | 'knowledge'>('commands')
  const [selectedSlashCommand, setSelectedSlashCommand] = useState<SlashCommand | null>(null)
  const [mentionOpen, setMentionOpen] = useState(false)
  const submitTimer = useRef<number | null>(null)
  const cancelPendingRef = useRef(false)
  const questionSubmittingRef = useRef(false)
  const composerRef = useRef<HTMLDivElement | null>(null)
  const promptRef = useRef<HTMLDivElement | null>(null)
  const commandMenuRef = useRef<HTMLDivElement | null>(null)
  const attachmentDialogRef = useRef<() => void>(() => undefined)

  useEffect(() => () => {
    if (submitTimer.current !== null) window.clearTimeout(submitTimer.current)
  }, [])

  useEffect(() => {
    if (submitTimer.current !== null) {
      window.clearTimeout(submitTimer.current)
      submitTimer.current = null
    }
    setStatus('ready')
    setCommandOpen(false)
    setCommandView('commands')
    setSelectedSlashCommand(null)
    setMentionOpen(false)
  }, [resetKey])

  useEffect(() => {
    setModel(defaultModel)
  }, [defaultModel, activeAgent?.id])

  useEffect(() => {
    setQuestionSelections({})
    setQuestionOtherAnswers({})
    setQuestionFreeform('')
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
        const composerTop = composer.getBoundingClientRect().top
        onHeightChange(Math.ceil(Math.max(0, stageBottom - composerTop + 6)))
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
    const leadingValue = value.trimStart()
    const hasSlashCommand = /^\/[^\s]*$/u.test(leadingValue)
    setDraft(value)
    setCommandOpen(hasSlashCommand)
    if (hasSlashCommand) setCommandView('commands')
    setMentionOpen(!hasSlashCommand && /(^|\s)@[^\s@]*$/u.test(value))
  }

  const applySlashCommand = (command: SlashCommand) => {
    setDraft((value) => value.replace(/^\s*\/[^\s]*/u, '').trimStart())
    setMentionOpen(false)
    if (command.action === 'attachments') {
      setCommandOpen(false)
      setCommandView('commands')
      setSelectedSlashCommand(null)
      attachmentDialogRef.current()
      return
    }
    if (command.action === 'knowledge') {
      setSelectedSlashCommand(null)
      setCommandView('knowledge')
      setCommandOpen(true)
      runtimeContext.onKnowledgeMenuOpen?.()
      return
    }
    setCommandOpen(false)
    setCommandView('commands')
    setSelectedSlashCommand(command)
  }

  const applyMention = (path: string) => {
    setDraft((value) => value.replace(/@[^\s@]*$/u, `@${path} `))
    setMentionOpen(false)
  }

  const onComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (commandOpen && (event.key === 'ArrowDown' || event.key === 'ArrowUp' || (event.key === 'Enter' && !event.nativeEvent.isComposing))) {
      event.preventDefault()
      commandMenuRef.current?.dispatchEvent(new globalThis.KeyboardEvent('keydown', {
        key: event.key,
        bubbles: true,
        cancelable: true,
      }))
      return
    }
    if (event.key === 'Escape') {
      setCommandOpen(false)
      setCommandView('commands')
      setMentionOpen(false)
    }
    if (event.key === 'Enter' && sendWithModifier) {
      if (event.ctrlKey || event.metaKey) return
      event.preventDefault()
      const textarea = event.currentTarget
      textarea.setRangeText('\n', textarea.selectionStart, textarea.selectionEnd, 'end')
      setDraft(textarea.value)
    }
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

  const submitQuestionAnswer = async () => {
    if (!activeQuestionRequest || questionSubmittingRef.current) return
    const answer = buildQuestionAnswer(questionFreeform)
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
    setQuestionFreeform('')
    setQuestionSelections({})
    setQuestionOtherAnswers({})
    onStatusChange?.('streaming')
  }

  const pendingApprovals = runtimeApprovals
    .filter((approval) => approval.status === 'pending')
    .sort((left, right) => left.requestedAt - right.requestedAt || left.id.localeCompare(right.id))
  const activeApproval = pendingApprovals[0]
  const demoApprovalPending = !runtimeControlled && chatState === 'approval' && Boolean(onApprove && onDeny)
  const demoQuestionPending = !runtimeControlled && chatState === 'question' && Boolean(onAnswer)
  const goalConfirmationPending = goalProgressData?.goal.status === 'proposed' && Boolean(onResolveWorkModeConfirmation)
  const planRevisionPending = Boolean(
    onResolvePlanRevision && goalProgressData?.planRevisions.some((plan) => plan.status === 'proposed'),
  )
  const decisionPending = demoApprovalPending || demoQuestionPending || pendingApprovals.length > 0 || Boolean(activeQuestionRequest) || goalConfirmationPending || planRevisionPending
  const approvalDecisionPending = demoApprovalPending || pendingApprovals.length > 0

  return (
    <div ref={composerRef} className={`fox-composer-wrap ${centered ? 'is-empty' : ''}`}>
      {(showGoal || goalProgressData) && <GoalFloater chatState={chatState} data={goalProgressData} onDelete={onDeleteGoal} onRunningChange={onGoalRunningChange} onEvidenceClick={onEvidenceClick} onResolveConfirmation={onResolveWorkModeConfirmation} onResolvePlanRevision={onResolvePlanRevision} onResolveWorkflowGate={onResolveWorkflowGate} />}
      <div ref={promptRef} className={`fox-prompt-shell ${decisionPending ? 'is-decision' : ''} ${approvalDecisionPending ? 'is-approval' : ''}`} onFocusCapture={() => setFocused(true)} onBlurCapture={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false) }}>
        {decisionPending ? <div className="fox-decision-card">
          {demoApprovalPending && onApprove && onDeny && <ApprovalPrompt onApprove={onApprove} onDeny={onDeny} />}
          {activeApproval && resolveRuntimeApproval && <div className="fox-decision-approval-list">
            <RuntimeApprovalPrompt key={activeApproval.id} approval={activeApproval} onResolve={resolveRuntimeApproval} />
          </div>}
          {pendingApprovals.length === 0 && !demoApprovalPending && demoQuestionPending && onAnswer && <QuestionPrompt onAnswer={onAnswer} />}
          {pendingApprovals.length === 0 && !demoApprovalPending && !demoQuestionPending && activeQuestionRequest && <div className="fox-question-decision-content">
            <header><span><CircleHelp size={15} /></span><div><strong>需要你补充一点信息</strong><small>回答后 Fox 会继续刚才的任务</small></div></header>
            <div className="fox-question-decision-list">
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
            <div className="fox-question-decision-actions">
              {activeQuestionRequest.questions.length === 1 && activeQuestionRequest.questions[0]?.allowOther && <Input disabled={questionSubmitting} value={questionFreeform} onChange={(event) => setQuestionFreeform(event.currentTarget.value)} placeholder="也可以输入自己的答案" />}
              <Button disabled={questionSubmitting} onClick={() => void submitQuestionAnswer()}>{questionSubmitting ? '正在提交…' : '确认并继续'}</Button>
            </div>
          </div>}
          {pendingApprovals.length === 0 && !demoApprovalPending && !demoQuestionPending && !activeQuestionRequest && (goalConfirmationPending || planRevisionPending) && goalProgressData && <div className="fox-goal-decision-content">
            <GoalProgress data={goalProgressData} defaultExpanded={false} onEvidenceClick={onEvidenceClick} onResolveConfirmation={onResolveWorkModeConfirmation} onResolvePlanRevision={onResolvePlanRevision} onResolveWorkflowGate={onResolveWorkflowGate} />
          </div>}
        </div> : <BorderBeam
          active={status === 'streaming' || (centered && focused)}
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
        {commandOpen && <Command ref={commandMenuRef} loop className={`fox-slash-menu ${commandView === 'knowledge' ? 'is-knowledge' : ''}`}>
          <CommandList>
            {commandView === 'commands' ? <>
              <CommandEmpty>没有匹配的命令</CommandEmpty>
              <CommandGroup>
                {slashCommands.map((command) => <CommandItem key={command.id} value={command.id} onSelect={() => applySlashCommand(command)}><span className="fox-command-icon">{command.icon}</span><span className="fox-command-copy"><strong>/{command.id}</strong><small>{command.title} · {command.detail}</small></span></CommandItem>)}
              </CommandGroup>
            </> : <>
              <CommandEmpty>{runtimeContext.knowledgeLoading ? '正在读取知识库…' : runtimeContext.knowledgeError || '当前没有可使用的知识库'}</CommandEmpty>
              <CommandGroup heading="远程知识库">
                {(runtimeContext.remoteKnowledgeBases ?? []).map((item) => {
                  const reference: KnowledgeReference = { source: 'remote', providerKey: 'yuxi-primary', connectionId: 'yuxi-primary', id: item.id }
                  const selected = selectedKnowledgeReferenceKeys.has(knowledgeReferenceKey(reference))
                  return <CommandItem key={`remote:${item.id}`} value={`远程知识库 ${item.name} ${item.description}`} onSelect={() => { if (!runtimeContext.knowledgeLoading) void runtimeContext.onKnowledgeToggle?.(reference, item.name) }}><span className="fox-command-icon"><Library /></span><span className="fox-command-copy"><strong>{item.name}</strong><small>{item.description || `${item.fileCount} 个文件`}</small></span>{selected && <Check className="fox-command-selected-check" />}</CommandItem>
                })}
              </CommandGroup>
              <div className="fox-slash-knowledge-separator" aria-hidden="true" />
              <CommandGroup heading="本地知识库">
                {(runtimeContext.localKnowledgeBases ?? []).map((item) => {
                  const reference: KnowledgeReference = { source: 'local', providerKey: 'local', id: item.id }
                  const selected = selectedKnowledgeReferenceKeys.has(knowledgeReferenceKey(reference))
                  const availability = localKnowledgePickerState(item)
                  return <CommandItem key={`local:${item.id}`} disabled={!selected && !availability.available} value={`本地知识库 ${item.name} ${item.description ?? ''}`} onSelect={() => { if (!runtimeContext.knowledgeLoading && (selected || availability.available)) void runtimeContext.onKnowledgeToggle?.(reference, item.name) }}><span className="fox-command-icon"><Library /></span><span className="fox-command-copy"><strong>{item.name}</strong><small>{item.description || `${item.documentCount} 个文档`} · {availability.status}</small></span>{selected && <Check className="fox-command-selected-check" />}</CommandItem>
                })}
              </CommandGroup>
            </>}
          </CommandList>
        </Command>}
        {mentionOpen && <div className="fox-mention-menu"><div className="fox-command-title"><span>引用工作区文件</span><kbd>@</kbd></div>{['docs/FOX_ARCHITECTURE.md', 'apps/desktop/src/features/chat/workbench.tsx', 'apps/desktop/src/styles/workbench.css'].map((path) => <button type="button" key={path} onMouseDown={(event) => event.preventDefault()} onClick={() => applyMention(path)}><FileText size={14} /><span>{path}</span></button>)}</div>}
        <PromptInput
          accept={supportsImageInput ? 'image/*,.pdf,.txt,.md,.docx,.xls,.xlsx,.ppt,.pptx,.csv,.tsv' : '.pdf,.txt,.md,.docx,.xls,.xlsx,.ppt,.pptx,.csv,.tsv'}
          multiple
          maxFiles={8}
          maxFileSize={5 * 1024 * 1024}
          onError={({ code }) => {
            if (code === 'max_file_size') toast.error('单个附件不能超过 5 MB')
            else if (code === 'max_files') toast.error('一次最多添加 8 个附件')
            else toast.error('支持 PDF、TXT、MD、DOCX、XLSX、PPTX、CSV、TSV；旧版 DOC、XLS、PPT 请先转换格式')
          }}
          onSubmitStart={({ text, files }) => {
            if (runtimeInitializing) return
            const prompt = text.trim() || (files.length ? '请分析这些附件' : '')
            if (!prompt && !activeQuestionRequest) return
            if (!activeQuestionRequest) flushSync(() => onPromptCommit?.(prompt))
          }}
          onSubmit={async ({ text, files: submittedFiles }) => {
          if (runtimeInitializing) return
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
          const selectedModel = conversationModelOverride(activeAgent, model, yuxiModels)
          const prompt = text.trim() || '请分析这些附件'
          const runtimeText = selectedSlashCommand ? slashCommandRuntimeText(selectedSlashCommand, prompt) : undefined
          const outcome = await onSubmitPrompt?.(prompt, selectedModel, submittedFiles, runtimeText)
          if (outcome === 'question' || outcome === 'approval' || outcome === 'error') {
            setStatus('ready')
            onStatusChange?.('ready')
            return
          }
          setSelectedSlashCommand(null)
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
        <ComposerAttachmentCommandBridge openRef={attachmentDialogRef} />
        <ComposerAttachmentPreview projectRoot={runtimeControlled ? projectRoot : undefined} knowledgeChips={runtimeControlled ? knowledgeChips : []} onProject={onProject} onKnowledge={openKnowledge} />
        <PromptInputBody>
          <PromptInputTextarea value={draft} disabled={questionSubmitting} spellCheck={spellcheckEnabled} onChange={onDraftChange} onKeyDown={onComposerKeyDown} className="fox-prompt-textarea" placeholder={activeQuestionRequest ? '补充你的答案，或直接选择上方选项' : '给 Fox 发消息，输入 / 查看命令，@ 引用文件…'} />
        </PromptInputBody>
        <PromptInputFooter className="fox-prompt-toolbar">
          <PromptInputTools className="fox-composer-tools">
            <PromptInputActionMenu>
              <PromptInputActionMenuTrigger tooltip="添加内容" aria-label="添加内容"><Plus size={18} /></PromptInputActionMenuTrigger>
              <PromptInputActionMenuContent className="fox-add-menu">
                <PromptInputActionAddAttachments label="添加文件" />
                {supportsImageInput && <PromptInputActionAddAttachments label="添加图片" />}
                <PromptInputActionMenuItem disabled={!runtimeControlled} onSelect={() => onProject?.()}><FolderOpen />{projectRoot ? '更换项目' : '选择项目'}</PromptInputActionMenuItem>
                <PromptInputActionMenuItem disabled={!runtimeControlled || expertReadOnly} onSelect={onChangeExpert}><Sparkles />添加专家</PromptInputActionMenuItem>
                <PromptInputActionMenuItem onSelect={openKnowledge}><Library />{knowledgeChips.length ? `知识库（已选 ${knowledgeChips.length}）` : '选择知识库'}</PromptInputActionMenuItem>
              </PromptInputActionMenuContent>
            </PromptInputActionMenu>
            <PromptInputActionMenu>
              <PromptInputActionMenuTrigger className="fox-execution-control" disabled={!runtimeControlled} tooltip="执行方式"><Terminal size={14} /><span>{{ ask: '执行前询问', allow: '自动执行', read_only: '只读模式' }[projectPermissionMode ?? 'ask']}</span><ChevronDown size={13} /></PromptInputActionMenuTrigger>
              <PromptInputActionMenuContent className="fox-add-menu fox-execution-menu">
                {([['ask', '执行前询问'], ['allow', '自动执行'], ['read_only', '只读模式']] as const).map(([value, label]) => (
                  <PromptInputActionMenuItem key={value} onSelect={() => void onPermissionModeChange?.(value)}><Terminal /><span>{label}</span>{(projectPermissionMode ?? 'ask') === value && <Check className="fox-menu-check" />}</PromptInputActionMenuItem>
                ))}
              </PromptInputActionMenuContent>
            </PromptInputActionMenu>
            {selectedSlashCommand && <button type="button" className="fox-command-mode-chip" title={`退出${selectedSlashCommand.modeLabel}模式`} onClick={() => setSelectedSlashCommand(null)}><span className="fox-command-mode-icon">{selectedSlashCommand.icon}</span><span>{selectedSlashCommand.modeLabel}</span><span className="fox-command-mode-close"><X size={10} /></span></button>}
            {activeExpert && onViewExpert && onChangeExpert && onRemoveExpert && <ExpertBindingChip expert={activeExpert} readOnly={expertReadOnly} toolAvailability={expertToolAvailability} onView={onViewExpert} onChange={onChangeExpert} onRemove={onRemoveExpert} />}
          </PromptInputTools>
          <div className="fox-composer-submit">
            <PromptInputSelect value={activeAgent?.id ?? ''} onValueChange={(value) => value && value !== activeAgent?.id && onAgentChange?.(value)} disabled={!activeAgent}>
              <PromptInputSelectTrigger className="fox-agent-control"><Sparkles size={14} /><PromptInputSelectValue /></PromptInputSelectTrigger>
              <PromptInputSelectContent position="popper" side="top" align="end" sideOffset={6}>
                {activeAgent && !selectableAgents.some((item) => item.id === activeAgent.id) && <PromptInputSelectItem value={activeAgent.id} disabled>{activeAgent.name}</PromptInputSelectItem>}
                {selectableAgents.map((item) => <PromptInputSelectItem key={item.id} value={item.id} disabled={!item.available}>{item.name}</PromptInputSelectItem>)}
              </PromptInputSelectContent>
            </PromptInputSelect>
            <PromptInputSubmit
              status={status}
              disabled={runtimeInitializing && status === 'ready'}
              title={runtimeInitializing ? '正在准备默认专家' : undefined}
              onStop={() => {
                if (cancelPendingRef.current) return
                if (submitTimer.current !== null) {
                  window.clearTimeout(submitTimer.current)
                  submitTimer.current = null
                }
                cancelPendingRef.current = true
                void Promise.resolve(onCancel?.()).then((stopped) => {
                  if (stopped === false) return
                  setStatus('ready')
                  onStatusChange?.('ready')
                }).finally(() => {
                  cancelPendingRef.current = false
                })
              }}
              className="fox-send-button"
            >
              {runtimeInitializing ? <LoaderCircle className="animate-spin" size={17} /> : status === 'streaming' ? <CircleStop size={17} /> : <ArrowUp size={18} />}
            </PromptInputSubmit>
          </div>
        </PromptInputFooter>
        </PromptInput>
        </BorderBeam>}
      </div>
    </div>
  )
}

const MemoizedComposer = memo(Composer)

type KnowledgePickerItem = {
  reference: KnowledgeReference
  name: string
  description: string
  source: 'local' | 'remote'
  status?: string | null
  unavailable?: boolean
}

export function KnowledgeBindingDialog({ open, remoteItems, localItems, localLoading, remoteLoading, remoteError, onRetry, bindings, references, busy, error, onOpenChange, onConfirm }: {
  open: boolean
  remoteItems: KnowledgeBaseRecord[]
  localItems: LocalKnowledgeBaseDto[]
  localLoading: boolean
  remoteLoading: boolean
  remoteError?: string | null
  onRetry: () => void
  bindings: KnowledgeBindingRecord[]
  references?: KnowledgeReference[]
  busy: boolean
  error?: string | null
  onOpenChange: (open: boolean) => void
  onConfirm: (references: KnowledgeReference[], names: Record<string, string>) => void
}) {
  const [selected, setSelected] = useState<string[]>([])
  const [source, setSource] = useState<'local' | 'remote'>('local')
  const [query, setQuery] = useState('')
  const configuredReferences = Array.isArray(references)
    ? references
    : bindings.filter((item) => item.enabled).map(knowledgeReferenceFromLegacyBinding)
  const wasOpen = useRef(false)

  useEffect(() => {
    if (open && !wasOpen.current) setSelected(configuredReferences.map(knowledgeReferenceKey))
    wasOpen.current = open
  }, [bindings, open, references])

  useEffect(() => {
    if (!open) return
    setSource('local')
    setQuery('')
  }, [open])

  const pickerItems: KnowledgePickerItem[] = [
    ...remoteItems.map((item) => ({
      reference: {
        source: 'remote' as const,
        providerKey: 'yuxi-primary',
        connectionId: 'yuxi-primary',
        id: item.id,
      },
      name: item.name,
      description: item.description || `${item.fileCount} 个文件`,
      source: 'remote' as const,
      status: item.status,
    })),
    ...localItems.map((item) => ({
      reference: {
        source: 'local' as const,
        providerKey: 'local' as const,
        id: item.id,
      },
      name: item.name,
      description: [item.description || `${item.documentCount} 个文档`, localKnowledgePickerState(item).reason].filter(Boolean).join(' · '),
      source: 'local' as const,
      status: localKnowledgePickerState(item).status,
      unavailable: !localKnowledgePickerState(item).available,
    })),
  ]
  const knownReferenceKeys = new Set(pickerItems.map((item) => knowledgeReferenceKey(item.reference)))
  pickerItems.push(...configuredReferences
    .filter((reference) => !knownReferenceKeys.has(knowledgeReferenceKey(reference)))
    .map((reference) => ({
      reference,
      name: reference.id,
      description: '当前绑定但暂时无法从目录读取',
      source: reference.source,
      status: '不可用',
      unavailable: true,
    })))
  const itemByKey = new Map(pickerItems.map((item) => [knowledgeReferenceKey(item.reference), item]))
  const search = query.trim().toLocaleLowerCase()
  const visibleItems = filterKnowledgePickerItems(pickerItems, source, query)
  const loading = source === 'local' ? localLoading : remoteLoading
  const toggle = (reference: KnowledgeReference) => {
    const key = knowledgeReferenceKey(reference)
    setSelected((current) => current.includes(key)
      ? current.filter((item) => item !== key)
      : [...current, key])
  }
  const confirm = () => {
    const names: Record<string, string> = {}
    const selectedReferences = selected.map((key) => {
      const item = itemByKey.get(key)
      if (item) names[key] = item.name
      return item?.reference
    }).filter((reference): reference is KnowledgeReference => Boolean(reference))
    onConfirm(selectedReferences, names)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="fox-knowledge-binding-dialog">
        <DialogHeader>
          <DialogTitle>选择会话知识库</DialogTitle>
          <DialogDescription>仅所选知识库可用于当前会话。</DialogDescription>
        </DialogHeader>
        <Tabs value={source} onValueChange={(value) => setSource(value as 'local' | 'remote')} className="fox-knowledge-picker-tabs">
          <TabsList aria-label="知识库来源" className="fox-knowledge-picker-sources">
            <TabsTrigger value="local">本地 · {localItems.length}</TabsTrigger>
            <TabsTrigger value="remote">远程 · {remoteLoading ? '读取中' : remoteError ? '未连接' : remoteItems.length}{remoteError && <AlertTriangle aria-hidden="true" className="fox-knowledge-connection-warning" />}</TabsTrigger>
          </TabsList>
          <label className="fox-knowledge-picker-search">
            <Search aria-hidden="true" />
            <Input aria-label="搜索知识库" placeholder="搜索知识库" value={query} onChange={(event) => setQuery(event.target.value)} />
          </label>
          <TabsContent value={source} className="fox-knowledge-picker-panel">
            {source === 'remote' && remoteError && <div role="status" className="fox-knowledge-picker-notice"><AlertTriangle aria-hidden="true" /><div><strong>远程连接暂不可用</strong><p>{remoteError}</p><p>本地知识库仍可使用。</p></div><Button variant="ghost" disabled={remoteLoading} onClick={onRetry}>重试</Button></div>}
            <div className="fox-knowledge-picker-list" aria-label={source === 'local' ? '本地知识库' : '远程知识库'} aria-busy={loading}>
              {visibleItems.map((item) => {
                const key = knowledgeReferenceKey(item.reference)
                const checked = selected.includes(key)
                return <label key={key} className={`fox-knowledge-picker-row${checked ? ' is-checked' : ''}${item.unavailable ? ' is-unavailable' : ''}`}>
                  <input type="checkbox" checked={checked} disabled={busy || (item.unavailable && !checked)} onChange={() => toggle(item.reference)} aria-label={item.name} />
                  <span className="fox-knowledge-picker-copy"><strong>{item.name}</strong><span>{item.description}{!item.unavailable && item.status ? ` · ${item.status}` : ''}</span></span>
                  {item.unavailable && <span className="fox-knowledge-picker-status">{item.status}</span>}
                </label>
              })}
              {!visibleItems.length && <p className="fox-knowledge-picker-empty" role="status">{loading ? '正在读取知识库…' : search ? '没有匹配的知识库，请换个关键词。' : source === 'local' ? '暂无本地知识库，请先在知识库页面导入。' : remoteError ? '连接恢复后可查看远程知识库。' : '暂无可访问的远程知识库。'}</p>}
            </div>
            {source === 'local' && <div className="fox-knowledge-picker-hint"><Popover><PopoverTrigger asChild><Button type="button" variant="ghost" size="sm" aria-label="为什么知识库暂不可选"><CircleHelp aria-hidden="true" />选择条件</Button></PopoverTrigger><PopoverContent className="max-w-80 text-sm" align="start"><strong>导入文档后，等待解析完成即可选择</strong><p>关键词检索可用的知识库也能加入聊天，无须安装向量模型。若显示待解析、暂停或失败，请到知识库页面查看任务进度和错误原因。</p></PopoverContent></Popover><span>有可检索内容即可加入聊天。</span><Button type="button" variant="ghost" size="sm" disabled={loading} onClick={onRetry}>刷新</Button></div>}
          </TabsContent>
        </Tabs>
        {error && <p role="alert" className="fox-knowledge-picker-error">{error}</p>}
        <DialogFooter className="fox-knowledge-picker-footer"><span aria-live="polite">已选 <strong>{selected.length}</strong> 个</span><div><Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={busy} onClick={confirm}>{busy && <LoaderCircle className="animate-spin" />}保存选择</Button></div></DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function ConversationManagementDialogs({ conversation, mode, busy, error, onClose, onRename, onDelete }: { conversation: { id: string; title: string } | null; mode: 'rename' | 'purge' | null; busy: boolean; error: string | null; onClose: () => void; onRename: (title: string) => void; onDelete: () => void }) {
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
    <Dialog open={mode === 'purge'} onOpenChange={(open) => { if (!open) onClose() }}>
      <DialogContent className="fox-conversation-dialog fox-delete-dialog">
        <DialogHeader><DialogTitle>永久删除对话？</DialogTitle><DialogDescription>“{conversation?.title}”及其消息、工具与审批记录、Fox 内部附件和未导出产物、Runtime Session，以及对应的远程线程（如有）将被永久删除。已导出到项目目录的文件不会受影响。此操作无法撤销。</DialogDescription></DialogHeader>
        {error && <p className="fox-conversation-dialog-error" role="alert">{error}</p>}
        <DialogFooter><Button variant="outline" onClick={onClose}>取消</Button><Button variant="destructive" disabled={busy} onClick={onDelete}>{busy && <LoaderCircle className="animate-spin" />}永久删除</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}

function ProjectDeleteDialog({ project, busy, onClose, onDelete }: { project: { name: string; root: string; items: Array<{ id: string; title: string }> } | null; busy: boolean; onClose: () => void; onDelete: () => void }) {
  return <Dialog open={Boolean(project)} onOpenChange={(open) => { if (!open) onClose() }}>
    <DialogContent className="fox-conversation-dialog">
      <DialogHeader>
        <DialogTitle>删除项目？</DialogTitle>
        <DialogDescription>“{project?.name}”及其 {project?.items.length ?? 0} 个对话、消息、工具记录和 Fox 内部产物将被删除。这里只会移除 Fox 中的项目记录，不会删除本机文件夹“{project?.root}”。此操作无法撤销。</DialogDescription>
      </DialogHeader>
      <DialogFooter><Button variant="outline" onClick={onClose}>取消</Button><Button variant="destructive" disabled={busy} onClick={onDelete}>{busy && <LoaderCircle className="animate-spin" />}删除项目</Button></DialogFooter>
    </DialogContent>
  </Dialog>
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

function normalizedProjectPath(value: string) {
  return value.replace(/\\/g, '/').replace(/^\.\//, '').replace(/\/{2,}/g, '/').replace(/\/$/, '')
}

function projectDisplayPath(value: string, projectRoot: string) {
  const normalizedValue = normalizedProjectPath(value)
  const normalizedRoot = normalizedProjectPath(projectRoot)
  const valueKey = normalizedValue.toLocaleLowerCase()
  const rootKey = normalizedRoot.toLocaleLowerCase()
  const relativePath = valueKey === rootKey
    ? ''
    : valueKey.startsWith(`${rootKey}/`)
      ? normalizedValue.slice(normalizedRoot.length + 1)
      : normalizedValue
  return `/${relativePath.replace(/^\/+/, '')}`
}

function createOpenFileTab(path: string, artifactId: string | null = null): OpenFileTab {
  const normalizedPath = normalizedProjectPath(path)
  return {
    id: normalizedPath.toLocaleLowerCase(),
    path: normalizedPath,
    name: normalizedPath.split('/').at(-1) ?? normalizedPath,
    artifactId,
  }
}

function artifactProjectPath(artifact: ArtifactRecord, projectRoot: string, entries: ProjectFileEntry[]) {
  const storagePath = normalizedProjectPath(artifact.storagePath)
  const normalizedRoot = normalizedProjectPath(projectRoot)
  const storageKey = storagePath.toLocaleLowerCase()
  const rootKey = normalizedRoot.toLocaleLowerCase()
  const relativePath = storageKey.startsWith(`${rootKey}/`)
    ? storagePath.slice(normalizedRoot.length + 1)
    : storagePath
  const relativeKey = relativePath.toLocaleLowerCase()
  return entries.find((entry) => {
    if (entry.isDirectory) return false
    const entryKey = normalizedProjectPath(entry.path).toLocaleLowerCase()
    return entryKey === relativeKey || storageKey.endsWith(`/${entryKey}`)
  })?.path ?? relativePath
}

function ProjectFilePreviewPane({ conversationId, path, displayPath }: { conversationId: string; path: string; displayPath: string }) {
  const [preview, setPreview] = useState<ProjectFilePreview | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [applications, setApplications] = useState<ArtifactApplication[]>([])
  const [busyAction, setBusyAction] = useState<ProjectFileActionResponse['action'] | 'project_folder' | null>(null)

  useEffect(() => {
    let cancelled = false
    setPreview(null)
    setLoading(true)
    setError(null)
    void desktopClient.readProjectFile(conversationId, path)
      .then((response) => { if (!cancelled) setPreview(response) })
      .catch((cause) => { if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    void desktopClient.projectFileAction(conversationId, path, 'inspect')
      .then((response) => { if (!cancelled) setApplications(response.applications) })
      .catch(() => { if (!cancelled) setApplications([]) })
    return () => { cancelled = true }
  }, [conversationId, path])

  const runAction = async (action: ProjectFileActionResponse['action'], applicationId?: string) => {
    if (busyAction || action === 'inspect') return
    setBusyAction(action)
    try {
      const response = await desktopClient.projectFileAction(conversationId, path, action, applicationId)
      setApplications(response.applications)
      if (action === 'copy_path') toast.success('已复制受控路径')
    } catch (cause) {
      toast.error(desktopErrorDetails(cause).message)
    } finally {
      setBusyAction(null)
    }
  }

  const openProjectFolder = async () => {
    if (busyAction) return
    setBusyAction('project_folder')
    try {
      await desktopClient.openProjectFolder(conversationId)
    } catch (cause) {
      toast.error(desktopErrorDetails(cause).message)
    } finally {
      setBusyAction(null)
    }
  }

  return (
    <div className="fox-file-preview fox-project-file-preview">
      <div className="fox-project-file-toolbar">
        <div className="fox-file-location" title={displayPath}><span>{displayPath}</span></div>
        <div className="fox-project-file-actions">
          <Tooltip>
            <TooltipTrigger asChild><button type="button" aria-label="打开项目文件夹" disabled={Boolean(busyAction)} onClick={() => void openProjectFolder()}><FolderOpen size={15} /></button></TooltipTrigger>
            <TooltipContent>打开项目文件夹</TooltipContent>
          </Tooltip>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm" variant="outline" className="fox-file-open-trigger" aria-label="打开方式" disabled={Boolean(busyAction)}><ExternalLink className="fox-file-open-icon" /><span>打开</span><ChevronDown className="fox-file-open-chevron" /></Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="fox-result-open-menu">
              <DropdownMenuLabel>文件操作</DropdownMenuLabel>
              <DropdownMenuItem onSelect={() => void runAction('open')}><ExternalLink size={14} />系统默认方式打开</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void runAction('reveal')}><FolderOpen size={14} />打开所在位置</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void runAction('copy_path')}><Copy size={14} />复制文件路径</DropdownMenuItem>
              <DropdownMenuSeparator />
              {applications.length > 0
                ? applications.map((application) => <DropdownMenuItem key={application.id} onSelect={() => void runAction('open_with', application.id)}><Terminal size={14} />{application.label}</DropdownMenuItem>)
                : <DropdownMenuItem disabled>未检测到已支持的编辑器</DropdownMenuItem>}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <div className="fox-project-file-content">
        {loading
          ? <div className="fox-panel-loading"><LoaderCircle className="animate-spin" />正在读取文件</div>
          : error
            ? <div className="fox-result-unavailable"><AlertTriangle size={21} /><strong>无法预览文件</strong><span>{error}</span></div>
            : /\.html?$/i.test(path) && preview?.content ? <HtmlFilePreview key={path} name={displayPath} content={preview.content} /> : <pre>{preview?.content ?? ''}</pre>}
        {preview?.truncated && <small className="fox-result-truncated">预览已截断，仅显示文件开头部分。</small>}
      </div>
    </div>
  )
}

function ArtifactFilePreview({ detail, artifactId, displayPath }: { detail: ConversationDetail | null; artifactId?: string | null; displayPath: string }) {
  const conversationId = detail?.conversation.id ?? null
  const artifact = detail?.artifacts.find((item) => item.id === artifactId) ?? null
  const [inspection, setInspection] = useState<ArtifactInspectResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const [busyAction, setBusyAction] = useState<ArtifactActionResponse['action'] | 'project_folder' | null>(null)
  const [error, setError] = useState<DesktopErrorDetails | null>(null)
  const [feedback, setFeedback] = useState('')

  useEffect(() => {
    if (!conversationId || !artifact) {
      setInspection(null)
      setError(null)
      setFeedback('')
      return
    }
    let cancelled = false
    setInspection(null)
    setLoading(true)
    setError(null)
    setFeedback('')
    void desktopClient.inspectArtifact(conversationId, artifact.id)
      .then((response) => { if (!cancelled) setInspection(response) })
      .catch((cause) => { if (!cancelled) setError(desktopErrorDetails(cause)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [artifact?.id, conversationId])

  const runAction = async (action: ArtifactActionResponse['action'], applicationId?: string) => {
    if (!conversationId || !artifact || busyAction) return
    setBusyAction(action)
    setError(null)
    setFeedback('')
    try {
      const response = await desktopClient.artifactAction(conversationId, artifact.id, action, applicationId)
      setInspection(response)
      const application = response.applications.find((item) => item.id === applicationId)
      setFeedback(
        action === 'preview'
          ? '已刷新文件预览'
          : action === 'open'
            ? '已使用系统默认方式打开'
            : action === 'open_with'
              ? application
                ? '已使用 ' + application.label + ' 打开'
                : '已交给编程软件打开'
              : action === 'reveal'
                ? '已在文件夹中定位'
                : '已复制受控路径'
      )
    } catch (cause) {
      setError(desktopErrorDetails(cause))
    } finally {
      setBusyAction(null)
    }
  }

  const openProjectFolder = async () => {
    if (!conversationId || busyAction) return
    setBusyAction('project_folder')
    setError(null)
    setFeedback('')
    try {
      await desktopClient.openProjectFolder(conversationId)
      setFeedback('已打开项目文件夹')
    } catch (cause) {
      setError(desktopErrorDetails(cause))
    } finally {
      setBusyAction(null)
    }
  }

  if (!artifact) {
    return <div className="fox-empty-panel"><FileText size={29} /><strong>从对话中选择文件</strong><span>每次回答产生的新建或修改文件会显示在回答底部，点击后在这里打开。</span></div>
  }

  const applications = inspection?.applications ?? []
  return (
    <div className="fox-result-panel is-single-file">
      <div className="fox-result-file-toolbar">
        <div className="fox-file-location" title={displayPath}><span>{displayPath}</span></div>
        <div className="fox-result-file-actions">
          <Tooltip>
            <TooltipTrigger asChild>
              <button type="button" aria-label="打开项目文件夹" disabled={loading || Boolean(busyAction) || !detail?.conversation.projectRoot} onClick={() => void openProjectFolder()}><FolderOpen size={15} /></button>
            </TooltipTrigger>
            <TooltipContent>打开项目文件夹</TooltipContent>
          </Tooltip>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm" variant="outline" className="fox-file-open-trigger" aria-label="打开方式" disabled={loading || Boolean(busyAction)}><ExternalLink className="fox-file-open-icon" /><span>打开</span><ChevronDown className="fox-file-open-chevron" /></Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="fox-result-open-menu">
              <DropdownMenuLabel>文件操作</DropdownMenuLabel>
              <DropdownMenuItem disabled={!inspection?.capabilities.open} onSelect={() => void runAction('open')}><ExternalLink size={14} />系统默认方式打开</DropdownMenuItem>
              <DropdownMenuItem disabled={!inspection?.capabilities.reveal} onSelect={() => void runAction('reveal')}><FolderOpen size={14} />打开所在位置</DropdownMenuItem>
              <DropdownMenuItem disabled={!inspection?.capabilities.copyPath} onSelect={() => void runAction('copy_path')}><Copy size={14} />复制文件路径</DropdownMenuItem>
              <DropdownMenuSeparator />
              {applications.length > 0
                ? applications.map((application) => <DropdownMenuItem key={application.id} onSelect={() => void runAction('open_with', application.id)}><Terminal size={14} />{application.label}</DropdownMenuItem>)
                : <DropdownMenuItem disabled>未检测到已支持的编辑器</DropdownMenuItem>}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <div className="fox-result-detail">
        {error && <div className="fox-result-error" role="alert"><AlertTriangle size={15} /><div><strong>{error.code}</strong><span>{error.message}</span>{error.retryable && <small>可以稍后重试。</small>}</div></div>}
        {feedback && <p className="fox-result-feedback" role="status">{feedback}</p>}
        <div className="fox-result-raw">
          {inspection?.preview.available && inspection.preview.content !== null
            ? inspection.preview.kind === 'html' ? <HtmlFilePreview key={artifact.id} name={artifact.displayName} content={inspection.preview.content} /> : <pre>{inspection.preview.content}</pre>
            : <div className="fox-result-unavailable"><FileText size={21} /><strong>{inspection?.preview.reason ?? (loading ? '正在请求 Host 预览…' : '暂时没有可用预览')}</strong><span>{inspection?.preview.kind === 'html' ? 'HTML 预览暂不可用，可使用顶部“打开方式”查看原文件。' : '可使用顶部“打开方式”查看原文件。'}</span></div>}
          {inspection?.preview.truncated && <small className="fox-result-truncated">预览已截断，仅显示前 512 KB。</small>}
        </div>
      </div>
    </div>
  )
}

function toolResultPreview(tool: ToolCallRecord) {
  if (tool.errorMessage) return tool.errorMessage
  if (typeof tool.result === 'string') return tool.result.trim().slice(0, 420)
  if (tool.result === null || tool.result === undefined) return '尚未返回结果'
  try {
    return JSON.stringify(tool.result, null, 2).slice(0, 420)
  } catch {
    return String(tool.result).slice(0, 420)
  }
}

function BrowserPanel({ detail }: { detail: ConversationDetail | null }) {
  const [activeTab, setActiveTab] = useState<'page' | 'activity'>('page')
  const [address, setAddress] = useState('')
  const [browserUrl, setBrowserUrl] = useState('')
  const [frameVersion, setFrameVersion] = useState(0)
  const webCalls = useMemo(
    () => (detail?.toolCalls ?? []).filter(isWebToolCall).sort((left, right) => right.startedAt - left.startedAt),
    [detail?.toolCalls],
  )

  const navigateTo = (value: string) => {
    const normalized = normalizeBrowserUrl(value)
    if (!normalized) {
      toast.error('请输入有效的 http 或 https 网页地址')
      return
    }
    setAddress(normalized)
    setBrowserUrl(normalized)
    setFrameVersion((version) => version + 1)
    setActiveTab('page')
  }
  const openExternal = async () => {
    if (!browserUrl) return
    try {
      await desktopClient.openExternalUrl(browserUrl)
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause))
    }
  }

  return (
    <WebPreview className="fox-browser-panel">
      <div className="fox-browser-tabs" role="tablist" aria-label="网页预览与网络记录">
        <button type="button" role="tab" aria-selected={activeTab === 'page'} className={activeTab === 'page' ? 'is-active' : ''} onClick={() => setActiveTab('page')}><Globe2 size={13} />手动预览</button>
        <button type="button" role="tab" aria-selected={activeTab === 'activity'} className={activeTab === 'activity' ? 'is-active' : ''} onClick={() => setActiveTab('activity')}><Activity size={13} />网络记录{webCalls.length > 0 && <small>{webCalls.length}</small>}</button>
      </div>
      <WebPreviewNavigation className="fox-browser-address">
        <WebPreviewNavigationButton disabled={!browserUrl} tooltip="刷新" onClick={() => setFrameVersion((version) => version + 1)}><RotateCcw size={14} /></WebPreviewNavigationButton>
        <form onSubmit={(event) => { event.preventDefault(); navigateTo(address) }}>
          <input aria-label="网页地址" value={address} onChange={(event) => setAddress(event.target.value)} placeholder="输入网址，例如 example.com" spellCheck={false} />
        </form>
        <WebPreviewNavigationButton disabled={!browserUrl} tooltip="在系统浏览器中打开" onClick={() => void openExternal()}><ExternalLink size={14} /></WebPreviewNavigationButton>
      </WebPreviewNavigation>
      {activeTab === 'page'
        ? browserUrl
          ? <div className="fox-browser-frame-wrap"><WebPreviewBody key={`${browserUrl}:${frameVersion}`} className="fox-browser-frame" src={browserUrl} /><p><ShieldCheck size={12} />网页运行在隔离预览中；若网站拒绝嵌入，请点击右上角外部打开。</p></div>
          : <div className="fox-browser-empty"><img className="fox-panel-mascot" src={FOX_ASSISTANT_AVATAR} alt="" /><strong>输入网址即可手动预览</strong><span>这里不能代替 Agent 操作网页；真实联网调用与结果保留在“网络记录”中。</span></div>
        : webCalls.length > 0
          ? <ScrollArea className="fox-browser-activity"><div>{webCalls.map((tool) => {
              const summary = webActivitySummary(tool)
              const status = tool.status === 'completed' ? '已完成' : tool.status === 'failed' ? '失败' : '执行中'
              const content = <><span className={`fox-browser-activity-icon is-${tool.status}`}><Globe2 size={14} /></span><div><strong>{summary.label}</strong><small>{summary.target}</small><pre>{toolResultPreview(tool)}</pre></div><Badge variant={tool.status === 'failed' ? 'destructive' : tool.status === 'completed' ? 'secondary' : 'outline'}>{status}</Badge>{summary.navigableUrl && <ChevronRight size={14} />}</>
              return summary.navigableUrl
                ? <button type="button" key={tool.id} onClick={() => navigateTo(summary.navigableUrl!)}>{content}</button>
                : <div key={tool.id} className="fox-browser-activity-row">{content}</div>
            })}</div></ScrollArea>
          : <div className="fox-browser-empty"><Globe2 size={30} /><strong>当前会话没有联网记录</strong><span>Agent 调用 web_search、web_read 或 http_request 后，目标、状态和结果会显示在这里。</span></div>}
    </WebPreview>
  )
}

function ChildRunStatusIcon({ run }: { run: ChildRunRecord }) {
  if (childRunIsActive(run)) return <LoaderCircle className="animate-spin" size={13} />
  if (run.status === 'completed') return <Check size={13} />
  if (run.status === 'cancelled' || run.status === 'interrupted') return <CircleStop size={13} />
  return <AlertTriangle size={13} />
}

type ChildTranscriptItem =
  | { kind: 'message'; id: string; createdAt: number; role: 'user' | 'assistant'; content: string }
  | { kind: 'tool'; id: string; createdAt: number; toolName: string; status: string }

function childTranscriptItems(detail: ConversationDetail | null): ChildTranscriptItem[] {
  if (!detail) return []
  const messages: ChildTranscriptItem[] = detail.messages
    .filter((message): message is ConversationMessage & { role: 'user' | 'assistant' } => (
      (message.role === 'user' || message.role === 'assistant') && Boolean(message.content.trim())
    ))
    .map((message) => ({
      kind: 'message',
      id: message.id,
      createdAt: message.createdAt,
      role: message.role,
      content: message.content.trim(),
    }))
  const tools: ChildTranscriptItem[] = detail.toolCalls.map((tool) => ({
    kind: 'tool',
    id: tool.id,
    createdAt: tool.startedAt,
    toolName: tool.toolName,
    status: tool.status,
  }))
  return [...messages, ...tools].sort((left, right) => left.createdAt - right.createdAt)
}

function childToolStatusLabel(status: string) {
  if (status === 'completed') return '已完成'
  if (status === 'failed') return '失败'
  return '执行中'
}

function childRunFailureTitle(run: ChildRunRecord) {
  if (/duration|timeout/i.test(run.errorCode ?? '') || /超时|time limit|timed out/i.test(run.errorMessage ?? '')) return '运行超时'
  if (run.status === 'interrupted') return '运行中断'
  return '运行失败'
}

function childRunFailureMessage(run: ChildRunRecord) {
  if (childRunFailureTitle(run) === '运行超时') {
    return run.budget.maxDurationMs
      ? `超过 ${formatDurationMs(run.budget.maxDurationMs)}执行上限，结果未采用`
      : '运行超过执行上限，结果未采用'
  }
  return run.errorMessage ?? (run.status === 'interrupted' ? '运行意外中断，本次结果未采用' : '子 Agent 未返回可采用结果')
}

function childAgentVisualQaEnabled() {
  return import.meta.env.DEV && new URLSearchParams(window.location.search).get('preview') === 'child-agent'
}

function childAgentVisualQaPanelWidth() {
  if (!childAgentVisualQaEnabled()) return null
  const requested = Number(new URLSearchParams(window.location.search).get('panelWidth'))
  return Number.isFinite(requested) && requested > 0 ? Math.min(760, Math.max(280, requested)) : 540
}

function childAgentVisualQaRuns(): ChildRunRecord[] {
  if (!childAgentVisualQaEnabled()) return []
  const now = Date.now()
  const base: ChildRunRecord = {
    id: 'preview-delegation-1',
    parentRunId: 'preview-parent-run',
    childRunId: 'preview-child-run-1',
    childConversationId: 'preview-conversation-1',
    workerAgentId: 'fox-debugger',
    workerAgentName: 'Fox 调试专家',
    objective: '审查 quicksort.py，找出所有 Bug，简洁输出，每个 Bug 一行标题 + 一行修复建议。',
    context: '请按 critical / high / medium / low 分级，并给出能够直接执行的修改建议。',
    teamRunId: null,
    teamMemberId: null,
    allowedTools: ['read_file', 'search_code', 'run_tests'],
    status: 'failed',
    depth: 1,
    budget: { maxDurationMs: 600_000, maxTotalTokens: 80_000, maxOutputTokens: 6_000, maxToolCalls: 12 },
    resultText: '已读取并审查代码，以下是按严重级别排列的 Bug 清单（节选）：\n\n• Critical：quicksort_inplace 递归深度无界，可能导致递归深度异常。\n• High：_sort 区间判断存在潜在偏差，缺少对 equal 分区的递归处理。\n• Medium：_inplace 缺少对空数组和单元素的提前返回保护。\n• Low：命名、注释和类型注解可以进一步统一。',
    inputTokens: 52_410,
    outputTokens: 12_570,
    totalTokens: 64_980,
    toolCallCount: 3,
    errorCode: 'runtime.duration_budget_exceeded',
    errorMessage: '超过 10 分钟执行上限，结果未采用',
    createdAt: now - 610_000,
    startedAt: now - 608_000,
    finishedAt: now - 8_000,
  }
  return [
    base,
    { ...base, id: 'preview-delegation-2', childRunId: 'preview-child-run-2', childConversationId: 'preview-conversation-2', objective: '审查 quicksort.py 代码，验证修复后的边界条件。', workerAgentName: 'Fox 代码审查专家', status: 'completed', resultText: '边界条件验证通过。', totalTokens: 18_420, toolCallCount: 5, errorCode: null, errorMessage: null, finishedAt: now - 180_000 },
    { ...base, id: 'preview-delegation-3', childRunId: 'preview-child-run-3', childConversationId: 'preview-conversation-3', objective: '对 quicksort.py 进行全面测试并记录失败用例。', workerAgentName: 'Fox 测试专家', status: 'completed', resultText: '已完成测试并返回用例摘要。', totalTokens: 21_760, toolCallCount: 7, errorCode: null, errorMessage: null, finishedAt: now - 420_000 },
    { ...base, id: 'preview-delegation-4', childRunId: 'preview-child-run-4', childConversationId: 'preview-conversation-4', objective: '检查排序实现的性能退化风险。', workerAgentName: 'Fox 性能专家', status: 'running', resultText: null, totalTokens: 8_320, toolCallCount: 2, errorCode: null, errorMessage: null, finishedAt: null },
  ]
}

const childAgentAvatars = [FOX_ASSISTANT_AVATAR, FOX_ASSISTANT_AVATAR, FOX_ASSISTANT_AVATAR, FOX_ASSISTANT_AVATAR]
const collapsedChildAgentLimit = 10

function ChildAgentPanel({ detail, onOpenChildAgent }: { detail: ConversationDetail | null; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void }) {
  const previewRuns = useMemo(childAgentVisualQaRuns, [])
  const runs = useMemo(
    () => [...(detail?.childRuns?.length ? detail.childRuns : previewRuns)].sort((left, right) => right.createdAt - left.createdAt),
    [detail?.childRuns, previewRuns],
  )
  const [showAllAgents, setShowAllAgents] = useState(false)
  const visibleRuns = showAllAgents ? runs : runs.slice(0, collapsedChildAgentLimit)

  return (
    <AIAgent className="fox-agent-panel">
      <AIAgentContent className="fox-agent-content">
        {runs.length === 0
          ? <div className="fox-agent-empty"><Bot size={27} /><strong>当前会话还没有子 Agent</strong><span>子 Agent 开始运行后，会显示在这个列表中。</span></div>
          : <nav className="fox-agent-list" aria-label="子 Agent 列表">
              {visibleRuns.map((run, index) => (
                <button type="button" key={run.childRunId} className="fox-agent-list-item" aria-label={`打开 ${run.workerAgentName} 消息`} onClick={() => onOpenChildAgent(run, index)}>
                  <img className="fox-agent-list-avatar" src={childAgentAvatars[index % childAgentAvatars.length]} alt="" />
                  <span className="fox-agent-list-copy"><strong>{run.workerAgentName}</strong><small title={run.objective}>{run.objective}</small></span>
                  <em className={`is-${run.status}`}>{childRunStatusLabel(run.status)}</em>
                  <ChevronRight className="fox-agent-list-open" aria-hidden="true" />
                </button>
              ))}
              {runs.length > collapsedChildAgentLimit && <button type="button" className="fox-agent-list-more" aria-expanded={showAllAgents} onClick={() => setShowAllAgents((value) => !value)}>
                <span>{showAllAgents ? '收起' : `显示更多（${runs.length - collapsedChildAgentLimit}）`}</span>
                {showAllAgents ? <ChevronUp /> : <ChevronDown />}
              </button>}
            </nav>}
      </AIAgentContent>
    </AIAgent>
  )
}

function childUserMessageNeedsCollapse(content: string) {
  return content.length > 84 || content.split(/\r?\n/).length > 3
}

function ChildAgentConversationPanel({ run, avatarIndex, onBack }: { run: ChildRunRecord; avatarIndex: number; onBack: () => void }) {
  const [childDetail, setChildDetail] = useState<ConversationDetail | null>(null)
  const [childDetailLoading, setChildDetailLoading] = useState(false)
  const [childDetailError, setChildDetailError] = useState<string | null>(null)
  const [expandedUserMessages, setExpandedUserMessages] = useState<Set<string>>(() => new Set())
  const transcript = useMemo(() => {
    const storedItems = childTranscriptItems(childDetail)
    if (storedItems.length || !run.childConversationId.startsWith('preview-conversation-')) return storedItems
    const startedAt = run.startedAt ?? run.createdAt
    const previewItems: ChildTranscriptItem[] = [{
      kind: 'message',
      id: `${run.childRunId}:objective`,
      createdAt: run.createdAt,
      role: 'user',
      content: run.objective,
    }]
    const tools = (run.allowedTools ?? []).slice(0, Math.max(1, Math.min(run.toolCallCount, 3)))
    tools.forEach((toolName, index) => previewItems.push({
      kind: 'tool',
      id: `${run.childRunId}:tool:${index}`,
      createdAt: startedAt + ((index + 1) * 30_000),
      toolName,
      status: childRunIsActive(run) && index === tools.length - 1 ? 'running' : 'completed',
    }))
    if (run.resultText) previewItems.push({
      kind: 'message',
      id: `${run.childRunId}:result`,
      createdAt: run.finishedAt ?? Date.now(),
      role: 'assistant',
      content: run.resultText,
    })
    return previewItems
  }, [childDetail, run])

  useEffect(() => {
    if (run.childConversationId.startsWith('preview-conversation-')) {
      setChildDetail(null)
      setChildDetailLoading(false)
      setChildDetailError(null)
      return
    }
    let cancelled = false
    let refreshTimer: number | undefined
    setChildDetail(null)
    setChildDetailLoading(true)
    setChildDetailError(null)
    const load = async (showLoading: boolean) => {
      if (showLoading) setChildDetailLoading(true)
      try {
        const value = await desktopClient.loadConversation(run.childConversationId)
        if (!cancelled) {
          setChildDetail(value)
          setChildDetailError(null)
        }
      } catch (cause) {
        if (!cancelled) setChildDetailError(desktopErrorDetails(cause).message)
      } finally {
        if (!cancelled && showLoading) setChildDetailLoading(false)
      }
    }
    void load(true)
    if (childRunIsActive(run)) refreshTimer = window.setInterval(() => { void load(false) }, 1_500)
    return () => {
      cancelled = true
      if (refreshTimer !== undefined) window.clearInterval(refreshTimer)
    }
  }, [run.childConversationId, run.status])

  return (
    <section className="fox-agent-conversation is-tab" aria-label={`${run.workerAgentName}只读消息记录`}>
      <header className="fox-agent-conversation-head">
        <button type="button" className="fox-agent-conversation-back" aria-label="返回子 Agent 列表" onClick={onBack}><ArrowLeft /></button>
        <div><strong title={run.objective}>{run.workerAgentName}</strong></div>
        <span className={`is-${run.status}`}><ChildRunStatusIcon run={run} />{childRunStatusLabel(run.status)}</span>
      </header>
      <div className="fox-agent-conversation-scroll">
        {childDetailLoading && !childDetail
          ? <div className="fox-agent-inline-state"><LoaderCircle className="animate-spin" />正在读取子 Agent 消息</div>
          : childDetailError && !childDetail
            ? <div className="fox-agent-inline-state is-error"><AlertTriangle />{childDetailError}</div>
            : transcript.map((item) => item.kind === 'tool'
              ? <div className={`fox-agent-tool-event is-${item.status}`} key={`tool:${item.id}`}>
                  {item.status === 'completed' ? <Check /> : item.status === 'failed' ? <AlertTriangle /> : <LoaderCircle className="animate-spin" />}
                  <span>{childToolStatusLabel(item.status)} · <code>{item.toolName}</code></span>
                </div>
              : item.role === 'user'
                ? <article className="fox-agent-message is-user" key={`message:${item.id}`}>
                    <div className="fox-agent-user-message">
                      <div className={`fox-agent-user-bubble ${childUserMessageNeedsCollapse(item.content) ? 'is-collapsible' : ''} ${expandedUserMessages.has(item.id) ? 'is-expanded' : ''}`}>
                        <p>{item.content}</p>
                        {childUserMessageNeedsCollapse(item.content) && <button type="button" className="fox-agent-message-toggle" aria-label={expandedUserMessages.has(item.id) ? '收起用户消息' : '展开用户消息'} aria-expanded={expandedUserMessages.has(item.id)} onClick={() => setExpandedUserMessages((current) => { const next = new Set(current); if (next.has(item.id)) next.delete(item.id); else next.add(item.id); return next })}>{expandedUserMessages.has(item.id) ? <ChevronUp /> : <ChevronDown />}</button>}
                      </div>
                      <time>{new Date(item.createdAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time>
                    </div>
                  </article>
                : <article className="fox-agent-message is-assistant" key={`message:${item.id}`}>
                    <img src={childAgentAvatars[avatarIndex % childAgentAvatars.length]} alt="" />
                    <div><small>{run.workerAgentName} · {new Date(item.createdAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</small><MarkdownResponse>{item.content}</MarkdownResponse></div>
                  </article>)}
        {!childDetailLoading && !childDetailError && transcript.length === 0 && <p className="fox-agent-transcript-empty">这个子 Agent 还没有可见消息。</p>}
        {childRunIsActive(run) && <div className="fox-agent-live-line"><LoaderCircle className="animate-spin" />子 Agent 正在运行，消息会自动更新</div>}
        {(run.status === 'failed' || run.status === 'interrupted') && <div className="fox-agent-run-error" role="alert">
          <AlertTriangle />
          <div><strong>{childRunFailureTitle(run)}</strong>{run.errorCode && <code>{run.errorCode}</code>}<p>{childRunFailureMessage(run)}</p></div>
        </div>}
      </div>
    </section>
  )
}

function ContextContent({ mode, detail, fileTabs, activeFileTabId, onOpenFile, onOpenEvidence, onOpenChildAgent }: { mode: RightMode; detail: ConversationDetail | null; fileTabs: OpenFileTab[]; activeFileTabId: string | null; onOpenFile: (path: string, artifactId: string | null) => void; onOpenEvidence?: (evidence: TaskEvidenceRecord) => void; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void }) {
  const conversationId = detail?.conversation.id ?? null
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
    }).catch((cause) => {
      if (!cancelled) setFileError(cause instanceof Error ? cause.message : String(cause))
    }).finally(() => { if (!cancelled) setFileLoading(false) })
    return () => { cancelled = true }
  }, [conversationId, mode])

  const activeFileTab = fileTabs.find((tab) => tab.id === activeFileTabId) ?? null
  const selectedArtifact = detail?.artifacts.find((item) => item.id === activeFileTab?.artifactId) ?? null

  if (mode === 'files') {
    const normalizedSearch = fileSearch.trim().toLowerCase()
    const visibleFiles = projectFiles.filter((item) => !item.isDirectory && (!normalizedSearch || item.path.toLowerCase().includes(normalizedSearch)))
    if (!conversationId || !detail?.conversation.projectRoot) return <div className="fox-empty-panel"><Folders size={29} /><strong>尚未选择项目</strong><span>为当前会话授权一个项目文件夹后即可浏览。</span></div>
    const selectFile = (path: string) => {
      const matchingArtifact = detail.artifacts.find((artifact) => artifactProjectPath(artifact, detail.conversation.projectRoot!, projectFiles) === path)
      onOpenFile(path, matchingArtifact?.id ?? null)
    }
    const openedFilePath = activeFileTab?.path ?? ''
    const openedFileDisplayPath = openedFilePath ? projectDisplayPath(openedFilePath, detail.conversation.projectRoot) : ''
    return (
      <div className={`fox-file-panel ${openedFilePath ? 'is-file-open' : 'is-tree-only'}`}>
        {openedFilePath
          ? selectedArtifact
            ? <ArtifactFilePreview detail={detail} artifactId={selectedArtifact.id} displayPath={openedFileDisplayPath} />
            : <ProjectFilePreviewPane conversationId={conversationId} path={openedFilePath} displayPath={openedFileDisplayPath} />
          : <div className="fox-file-tree-pane">
              <div className="fox-panel-search"><Search size={14} /><input value={fileSearch} onChange={(event) => setFileSearch(event.target.value)} placeholder="搜索文件" />{fileSearch && <button type="button" aria-label="清除搜索" onClick={() => setFileSearch('')}><X size={12} /></button>}</div>
              <ScrollArea className="fox-file-tree">
                {fileLoading ? <div className="fox-panel-loading"><LoaderCircle className="animate-spin" />正在读取项目文件</div> : normalizedSearch ? <div className="fox-file-search-results">{visibleFiles.map((item) => <button type="button" key={item.path} onClick={() => selectFile(item.path)}><FileText size={14} /><span><strong>{item.name}</strong><small>{item.parent || detail.conversation.projectRoot}</small></span></button>)}{visibleFiles.length === 0 && <p>没有匹配的文件</p>}</div> : projectFileTree(projectFiles, detail.conversation.projectRoot.split(/[\\/]/).at(-1) || detail.conversation.projectRoot, '', selectFile)}
              </ScrollArea>
              {fileError && <p className="fox-file-tree-error"><AlertTriangle size={14} />{fileError}</p>}
            </div>}
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
    const changeFiles = (detail?.toolCalls ?? []).filter((item) => isChangeOrCommandTool(item.toolName))
    const activeChange = changeFiles.find((item) => item.id === selectedChange) ?? changeFiles.at(-1)
    const isCommand = Boolean(activeChange && isCommandTool(activeChange.toolName))
    const diffLines = activeChange && !isCommand ? toolDiff(activeChange, detail?.approvals ?? []).split('\n').map((line) => [line.startsWith('+') ? 'added' : line.startsWith('-') ? 'removed' : line.startsWith('@@') ? 'meta' : 'context', line]) : []
    const command = activeChange && isCommand ? commandOutput(activeChange) : null
    const linkedEvidence = activeChange ? (detail?.evidence ?? []).filter((evidence) => evidence.refKind === 'tool_call' && (evidence.refId === activeChange.id || evidence.refId === activeChange.runtimeToolCallId)) : []
    if (!changeFiles.length) return <div className="fox-empty-panel"><FileEdit size={29} /><strong>当前会话没有文件更改或命令</strong><span>Fox 的写入、编辑和命令记录会显示在这里。</span></div>
    return (
      <div className="fox-change-panel">
        <div className="fox-change-list">
          {changeFiles.map((item) => <button type="button" key={item.id} className={activeChange?.id === item.id ? 'is-active' : ''} onClick={() => setSelectedChange(item.id)}>{isCommandTool(item.toolName) ? <Terminal size={14} /> : <FileEdit size={14} />}<span><strong>{isCommandTool(item.toolName) ? item.toolName : toolTarget(item)}</strong><small>{item.toolName} · {item.status === 'completed' ? '已完成' : item.status === 'failed' ? '失败' : '处理中'}</small></span></button>)}
        </div>
        <div className="fox-diff-view">
          <div className="fox-diff-head">{isCommand ? <Terminal size={14} /> : <FileEdit size={14} />}<strong>{activeChange ? (isCommand ? activeChange.toolName : toolTarget(activeChange)) : '更改详情'}</strong><span><Badge variant={activeChange?.status === 'failed' ? 'destructive' : 'outline'}>{activeChange?.status ?? '未知'}</Badge>{linkedEvidence.length > 0 && <Badge variant="secondary">{linkedEvidence.length} 条证据</Badge>}</span></div>
          {linkedEvidence.length > 0 && <div className="fox-linked-evidence">{linkedEvidence.map((evidence) => <button type="button" key={evidence.id} disabled={!onOpenEvidence} onClick={() => onOpenEvidence?.(evidence)}><ShieldCheck size={12} /><span>{evidence.summary}</span><small>{evidence.validityStatus}</small></button>)}</div>}
          {isCommand && activeChange && command ? <div className="fox-command-detail">
            <div className="fox-command-facts"><span>退出码 <b>{command.exitCode === null ? '—' : command.exitCode}</b></span><span>耗时 <b>{command.duration === null ? '进行中' : `${command.duration} ms`}</b></span>{command.cwd && <span title={command.cwd}>目录 <b>{command.cwd}</b></span>}</div>
            <ScrollArea className="fox-command-output"><pre>{command.output || activeChange.errorMessage || '命令没有返回输出。'}</pre></ScrollArea>
          </div> : <ScrollArea className="fox-diff-scroll"><code>{diffLines.map(([tone, line], index) => <span key={index} className={`is-${tone}`}><i>{index + 1}</i><b>{line || ' '}</b></span>)}</code></ScrollArea>}
        </div>
      </div>
    )
  }
  if (mode === 'todo') {
    const workTasks = detail?.tasks ?? []
    const toolCalls = detail?.toolCalls ?? []
    if (!workTasks.length && !toolCalls.length) return <div className="fox-empty-panel"><ListTodo size={29} /><strong>当前会话暂无待办</strong><span>创建计划任务或开始调用工具后，执行状态会显示在这里。</span></div>
    return (
      <div className="fox-todo-panel">
        <Queue className="fox-ai-queue">
          <QueueList className="fox-ai-queue-list">
            {workTasks.length > 0
              ? workTasks.map((item) => {
                  const done = item.status === 'completed' || item.status === 'skipped'
                  const active = item.status === 'in_progress'
                  const failed = item.status === 'blocked' || item.status === 'interrupted'
                  const statusLabel = item.status === 'completed' ? '已完成' : item.status === 'skipped' ? '已跳过' : active ? '进行中' : item.status === 'blocked' ? '已阻塞' : item.status === 'interrupted' ? '已中断' : '待处理'
                  return <QueueItem key={item.id} className="fox-ai-queue-item"><div className="fox-ai-queue-row"><QueueItemIndicator completed={done} className={active ? 'is-progress' : ''} /><QueueItemContent completed={done}>{item.title}</QueueItemContent><QueueItemActions><Badge variant={failed ? 'destructive' : done ? 'secondary' : 'outline'}>{statusLabel}</Badge></QueueItemActions></div>{(item.detail || item.blockedReason) && <QueueItemDescription completed={done}>{item.detail || item.blockedReason}</QueueItemDescription>}</QueueItem>
                })
              : toolCalls.map((item) => { const done = item.status === 'completed'; const active = !done && !['failed', 'denied', 'cancelled'].includes(item.status); return <QueueItem key={item.id} className="fox-ai-queue-item"><div className="fox-ai-queue-row"><QueueItemIndicator completed={done} className={active ? 'is-progress' : ''} /><QueueItemContent completed={done}>{toolActivity({ id: item.id, seq: 0, name: item.toolName, input: item.input, output: item.result, isError: item.status === 'failed', completed: done, awaitingUser: false })}</QueueItemContent><QueueItemActions><Badge variant={item.status === 'failed' ? 'destructive' : done ? 'secondary' : 'outline'}>{item.status === 'failed' ? '失败' : done ? '已完成' : active ? '进行中' : '待处理'}</Badge></QueueItemActions></div><QueueItemDescription completed={done}>{toolTarget(item)}</QueueItemDescription></QueueItem> })}
          </QueueList>
        </Queue>
      </div>
    )
  }
  if (mode === 'browser') {
    return <BrowserPanel detail={detail} />
  }
  if (mode === 'agents') {
    return <ChildAgentPanel detail={detail} onOpenChildAgent={onOpenChildAgent} />
  }
  return null
}

const rightItems: Array<[RightMode, string, ReactNode]> = [
  ['knowledge', '知识库', <BookOpen size={16} />],
  ['files', '文件', <Folders size={16} />],
  ['changes', '更改', <FileEdit size={16} />],
  ['todo', '待办', <ListTodo size={16} />],
  ['browser', '网页与网络', <Globe2 size={16} />],
  ['agents', '子 Agent', <Bot size={16} />]
]

function RightPanel({ mode, tabs, detail, width, compact = false, maximized = false, fileTabs, activeFileTabId, activeChildAgent, onMode, onCloseMode, onOpenFile, onActivateFile, onCloseFile, onOpenChildAgent, onBackChildAgent, onOpenEvidence, onToggleMaximized, onCollapse }: { mode: RightMode | null; tabs: RightMode[]; detail: ConversationDetail | null; width: number; compact?: boolean; maximized?: boolean; fileTabs: OpenFileTab[]; activeFileTabId: string | null; activeChildAgent: ChildAgentSelection | null; onMode: (mode: RightMode) => void; onCloseMode: (mode: RightMode) => void; onOpenFile: (path: string, artifactId: string | null) => void; onActivateFile: (tabId: string) => void; onCloseFile: (tabId: string) => void; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void; onBackChildAgent: () => void; onOpenEvidence?: (evidence: TaskEvidenceRecord) => void; onToggleMaximized: () => void; onCollapse: () => void }) {
  const activeChildAgentRun = activeChildAgent
    ? detail?.childRuns.find((run) => run.childRunId === activeChildAgent.run.childRunId) ?? activeChildAgent.run
    : null
  const homeItemCounts: Record<RightMode, number> = {
    knowledge: detail?.knowledgeBindings.filter((binding) => binding.enabled !== false).length ?? 0,
    files: detail?.artifacts.length ?? 0,
    changes: detail?.toolCalls.filter((item) => isChangeOrCommandTool(item.toolName)).length ?? 0,
    todo: detail?.tasks.length ?? 0,
    browser: detail?.toolCalls.filter(isWebToolCall).length ?? 0,
    agents: detail?.childRuns.length ?? 0,
  }
  return (
    <aside className={`fox-context-panel ${mode === 'files' ? 'is-files' : ''} ${compact ? 'is-compact' : ''} ${maximized ? 'is-maximized' : ''}`} style={compact || maximized ? undefined : { width }}>
      <div className="fox-context-tabbar">
        <div className="fox-context-tabs" role="tablist" aria-label="已打开的侧边栏页面">
          {tabs.map((tabMode) => {
            const item = rightItems.find(([id]) => id === tabMode)
            if (!item) return null
            const active = tabMode === mode && (tabMode !== 'files' || activeFileTabId === null)
            return <div key={tabMode} className={`fox-context-tab ${active ? 'is-active' : ''}`}>
              <button type="button" role="tab" aria-selected={active} title={item[1]} onClick={() => onMode(tabMode)}>{item[2]}<span>{item[1]}</span></button>
              <button type="button" className="fox-context-tab-close" aria-label={`关闭${item[1]}页面`} onClick={() => onCloseMode(tabMode)}><X size={12} /></button>
            </div>
          })}
          {fileTabs.map((tab) => {
            const active = mode === 'files' && activeFileTabId === tab.id
            return <div key={`file:${tab.id}`} className={`fox-context-tab is-file-tab ${active ? 'is-active' : ''}`}>
              <button type="button" role="tab" aria-selected={active} title={tab.path} onClick={() => onActivateFile(tab.id)}><FileText size={14} /><span>{tab.name}</span></button>
              <button type="button" className="fox-context-tab-close" aria-label={`关闭 ${tab.name}`} onClick={() => onCloseFile(tab.id)}><X size={12} /></button>
            </div>
          })}
        </div>
        <div className="fox-context-tab-actions">
          <DropdownMenu>
            <DropdownMenuTrigger asChild><button type="button" aria-label="打开侧边栏页面" title="打开侧边栏页面"><Plus size={14} /></button></DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="fox-context-add-menu">
              {rightItems.map(([id, label, icon]) => <DropdownMenuItem key={id} onSelect={() => onMode(id)}>{icon}<span>{label}</span></DropdownMenuItem>)}
            </DropdownMenuContent>
          </DropdownMenu>
          <button type="button" aria-label={maximized ? '还原右侧边栏' : '全屏显示右侧边栏'} title={maximized ? '还原' : '全屏'} onClick={onToggleMaximized}>{maximized ? <Minimize2 size={14} /> : <Maximize2 size={14} />}</button>
          <button type="button" className="is-sidebar-open" aria-label="关闭右侧边栏" title="关闭右侧边栏" onClick={onCollapse}><PanelRight size={14} /></button>
        </div>
      </div>
      {mode === 'agents' && activeChildAgent && activeChildAgentRun
        ? <ChildAgentConversationPanel key={activeChildAgentRun.childRunId} run={activeChildAgentRun} avatarIndex={activeChildAgent.avatarIndex} onBack={onBackChildAgent} />
        : mode
        ? <ContextContent mode={mode} detail={detail} fileTabs={fileTabs} activeFileTabId={activeFileTabId} onOpenFile={onOpenFile} onOpenEvidence={onOpenEvidence} onOpenChildAgent={onOpenChildAgent} />
        : <div className="fox-context-home"><nav aria-label="侧边栏功能">{rightItems.map(([id, label, icon]) => {
            const count = homeItemCounts[id]
            return <button type="button" key={id} onClick={() => onMode(id)}>{icon}<span>{label}</span>{count > 0 && <small>{count}</small>}</button>
          })}</nav></div>}
    </aside>
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

function ConversationSummaryPopover({ detail, modelName, usage, contextWindow, onOpenRightMode }: { detail: ConversationDetail | null; modelName: string; usage: ConversationUsage; contextWindow: number; onOpenRightMode: (mode: RightMode) => void }) {
  const [open, setOpen] = useState(false)
  const hasContextWindow = contextWindow > 0
  const contextPercent = hasContextWindow ? Math.min(100, Math.round((usage.totalTokens / contextWindow) * 100)) : 0
  const cacheEligibleTokens = usage.inputTokens + usage.cacheReadTokens
  const cacheHitRate = cacheEligibleTokens ? Math.round(usage.cacheReadTokens / cacheEligibleTokens * 100) : 0
  const artifacts = detail?.artifacts ?? []
  const tasks = detail?.tasks ?? []
  const activeProcesses = (detail?.toolCalls ?? []).filter((tool) => !['completed', 'failed', 'denied', 'cancelled'].includes(tool.status))
  const subAgents = detail?.childRuns ?? []
  const sources = runtimeProcess(detail?.runtimeEvents ?? []).sources
  const diagnostics = latestRuntimeDiagnostics(detail?.runtimeEvents ?? [])
  const diagnosticDetail = [
    diagnostics.provider || diagnostics.apiType,
    diagnostics.phase ? `阶段 ${diagnostics.phase}` : '',
    diagnostics.retries ? `重试 ${diagnostics.retries}` : '',
    diagnostics.compactions ? `压缩 ${diagnostics.compactions}` : '',
    diagnostics.plannerStatus,
  ].filter(Boolean).join(' · ')
  const summaryItems: Array<{ icon: ReactNode; label: string; count: number; detail: string; mode: RightMode }> = [
    { icon: <FileText size={15} />, label: '文件输出', count: artifacts.length, detail: artifacts.slice(0, 2).map((item) => item.displayName).join('、'), mode: 'files' },
    { icon: <ListTodo size={15} />, label: '任务计划', count: tasks.length, detail: tasks.slice(0, 2).map((item) => item.title).join('、'), mode: 'todo' },
    { icon: <Terminal size={15} />, label: '后台进程', count: activeProcesses.length, detail: activeProcesses.slice(0, 2).map((item) => item.toolName).join('、'), mode: 'todo' },
    { icon: <Bot size={15} />, label: '子专家', count: subAgents.length, detail: subAgents.slice(-2).map((item) => `${item.workerAgentName} · ${item.status}`).join('、'), mode: 'agents' },
    { icon: <BookOpen size={15} />, label: '来源', count: sources.length, detail: sources.slice(0, 2).map((item) => item.title).join('、'), mode: 'knowledge' },
    { icon: <Wrench size={15} />, label: '运行诊断', count: diagnostics.toolCount, detail: diagnosticDetail, mode: 'todo' },
  ]
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip>
        <TooltipTrigger asChild><PopoverTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button" aria-label="对话摘要"><Activity size={16} /></Button></PopoverTrigger></TooltipTrigger>
        <TooltipContent side="bottom"><p>对话摘要</p></TooltipContent>
      </Tooltip>
      <PopoverContent align="end" sideOffset={8} className="fox-conversation-summary">
        <div className="fox-summary-model">
          <div><span>模型</span><strong title={modelName}>{modelName}</strong></div>
          <div className="fox-summary-context-copy"><span>上下文</span><strong>{compactTokenCount(usage.totalTokens)} / {hasContextWindow ? compactTokenCount(contextWindow) : '未知'}</strong></div>
          <div className="fox-summary-context-track"><span style={{ width: `${contextPercent}%` }} /></div>
          <small>{hasContextWindow ? `${contextPercent}% 已使用` : '未配置上下文上限'} · 本次输入 {compactTokenCount(usage.inputTokens)} · 输出 {compactTokenCount(usage.outputTokens)}</small>
          <small>缓存读取 {compactTokenCount(usage.cacheReadTokens)} · 写入 {compactTokenCount(usage.cacheWriteTokens)} · 命中率 {cacheHitRate}%</small>
          {(diagnostics.provider || diagnostics.apiType) && <small title={diagnostics.baseUrl || undefined}>{[diagnostics.provider, diagnostics.apiType].filter(Boolean).join(' · ')}</small>}
        </div>
        <div className="fox-summary-list">
          {summaryItems.map((item) => <button type="button" key={item.label} className="fox-summary-item" aria-label={`打开${item.label}侧边栏`} onClick={() => { setOpen(false); onOpenRightMode(item.mode) }}>
            <span className="fox-summary-icon">{item.icon}</span>
            <span><strong>{item.label}</strong><small>{item.detail || '暂无'}</small></span>
            <span className="fox-summary-item-tail"><b>{item.count}</b><ChevronRight /></span>
          </button>)}
        </div>
      </PopoverContent>
    </Popover>
  )
}

function ChatTopbar({ rightSidebarCollapsed, onRightSidebarExpand, onOpenRightMode, conversation, detail, modelName, usage, contextWindow, state, onPinConversation, onRenameConversation, onArchiveConversation }: { rightSidebarCollapsed: boolean; onRightSidebarExpand: () => void; onOpenRightMode: (mode: RightMode) => void; conversation?: ConversationSummary | null; detail: ConversationDetail | null; modelName: string; usage: ConversationUsage; contextWindow: number; state: ChatState; onPinConversation: (conversation: ConversationSummary) => void; onRenameConversation: (conversation: ConversationSummary) => void; onArchiveConversation: (conversation: ConversationSummary) => void }) {
  const [projectPopoverOpen, setProjectPopoverOpen] = useState(false)
  const projectRoot = conversation?.projectRoot?.trim() ?? ''
  const normalizedProjectRoot = projectRoot.replace(/[\\/]+$/, '')
  const projectName = normalizedProjectRoot.split(/[\\/]/).at(-1) || '未绑定项目'
  const openProjectFolder = async () => {
    if (!conversation || !projectRoot) return
    try {
      await desktopClient.openProjectFolder(conversation.id)
      setProjectPopoverOpen(false)
    } catch (cause) {
      toast.error('无法打开项目文件夹', { description: cause instanceof Error ? cause.message : String(cause) })
    }
  }
  return (
    <header className="fox-chat-topbar">
      <div className="fox-workspace-heading">
        <div className="fox-chat-heading-main">
          <Popover open={projectPopoverOpen} onOpenChange={setProjectPopoverOpen}>
            <PopoverTrigger asChild><button type="button" className="fox-chat-project-trigger" aria-label="查看对话项目"><FolderOpen size={14} /></button></PopoverTrigger>
            <PopoverContent align="start" sideOffset={7} className="fox-chat-project-popover">
              <div className="fox-chat-project-row is-summary"><Folder size={15} /><strong>{projectName}</strong></div>
              <button type="button" className="fox-chat-project-row" disabled={!projectRoot} title={projectRoot || '当前对话未绑定项目目录'} onClick={() => void openProjectFolder()}><FolderOpen size={15} /><span>{projectRoot || '当前对话未绑定项目目录'}</span></button>
              <button type="button" className="fox-chat-project-row" disabled={!conversation} onClick={() => { if (!conversation) return; setProjectPopoverOpen(false); onRenameConversation(conversation) }}><Settings size={15} /><span>编辑名称</span></button>
            </PopoverContent>
          </Popover>
          <span className="fox-chat-title">{conversation?.title || '新对话'}</span>
          <DropdownMenu>
            <Tooltip>
              <TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button fox-chat-title-more" aria-label="更多"><MoreHorizontal size={16} /></Button></DropdownMenuTrigger></TooltipTrigger>
              <TooltipContent side="bottom"><p>更多</p></TooltipContent>
            </Tooltip>
            <DropdownMenuContent align="start" className="fox-top-menu">
              <DropdownMenuLabel>当前对话</DropdownMenuLabel>
              <DropdownMenuItem disabled={!conversation} onSelect={() => conversation && onPinConversation(conversation)}>{conversation?.pinned ? <PinOff /> : <Pin />}{conversation?.pinned ? '取消置顶' : '置顶对话'}</DropdownMenuItem>
              <DropdownMenuItem disabled={!conversation} onSelect={() => conversation && onRenameConversation(conversation)}><FileEdit />重命名对话</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem disabled={!conversation} onSelect={() => conversation && onArchiveConversation(conversation)}><Archive />归档对话</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <div className="fox-chat-top-actions">
        <ConversationSummaryPopover detail={detail} modelName={modelName} usage={usage} contextWindow={contextWindow} onOpenRightMode={onOpenRightMode} />
        {state === 'running' && <Badge className="fox-running-badge"><span />运行中</Badge>}
        {state === 'question' && <Badge className="fox-question-badge"><CircleHelp size={12} />等待回答</Badge>}
        {state === 'approval' && <Badge className="fox-approval-badge"><ShieldCheck size={12} />等待批准</Badge>}
        {state === 'error' && <Badge className="fox-error-badge"><AlertTriangle size={12} />运行错误</Badge>}
        {state === 'denied' && <Badge className="fox-denied-badge"><CircleStop size={12} />已取消</Badge>}
        {rightSidebarCollapsed && <IconButton label="展开右侧边栏" className="fox-right-sidebar-expand" tooltipSide="bottom" onClick={onRightSidebarExpand}><PanelRight size={14} /></IconButton>}
      </div>
    </header>
  )
}

export function Workbench() {
  const desktopConversation = useDesktopConversation()
  const yuxi = useYuxiService()
  const yuxiUser = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const { profile: userProfile } = useUserProfile({ name: yuxiUser.user?.username, avatar: yuxiUser.user?.avatar })
  const yuxiModels = useYuxiModels(Boolean(yuxi.service?.credentialConfigured))
  const modelService = useModelService()
  const agentResource = useAgents()
  const knowledge = useKnowledgeBases(Boolean(yuxi.service?.credentialConfigured))
  const [activeView, setActiveView] = useState<WorkspaceView>(() => typeof window !== 'undefined' && !window.localStorage.getItem('fox.onboarding.status') ? 'onboarding' : 'chat')
  const [localKnowledgeCreateRequest, setLocalKnowledgeCreateRequest] = useState(0)
  const [assistantMode, setAssistantMode] = useState<AssistantMode>('assistant')
  const [activeEntityId, setActiveEntityId] = useState<string | null>(null)
  const [activeDocumentId, setActiveDocumentId] = useState<string | null>(null)
  const [activeSourceLocator, setActiveSourceLocator] = useState<KnowledgeSourceLocator | null>(null)
  const managementReturnRoutes = useRef<ManagementReturnRoutes>({
    settings: { view: 'chat', entityId: null, documentId: null },
    knowledge: { view: 'chat', entityId: null, documentId: null },
  })
  const onboardingReturnRoute = useRef<{ view: WorkspaceView; entityId: string | null; documentId: string | null }>({ view: 'chat', entityId: null, documentId: null })
  useEffect(() => {
    void desktopConversation.refreshRuntimeStatus()
  }, [desktopConversation.refreshRuntimeStatus, modelService.service?.supportsImageInput])
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => readStoredBoolean(layoutStorage.sidebarCollapsed, false))
  const [sidebarWidth, setSidebarWidth] = useState(() => {
    const storedWidth = readStoredNumber(layoutStorage.sidebarWidth, 236, 220, 456)
    return [304, 292, 280, 268, 256].includes(storedWidth) ? 236 : storedWidth
  })
  const [rightMode, setRightMode] = useState<RightMode | null>(readStoredRightMode)
  const [openRightTabs, setOpenRightTabs] = useState<RightMode[]>(readStoredRightTabs)
  const [openFileTabs, setOpenFileTabs] = useState<OpenFileTab[]>([])
  const [activeFileTabId, setActiveFileTabId] = useState<string | null>(null)
  const [activeChildAgent, setActiveChildAgent] = useState<ChildAgentSelection | null>(null)
  const [rightSidebarCollapsed, setRightSidebarCollapsed] = useState(true)
  const [rightPanelMaximized, setRightPanelMaximized] = useState(false)
  const [rightWidth, setRightWidth] = useState(() => childAgentVisualQaPanelWidth() ?? readStoredNumber(layoutStorage.rightWidth, 360, 280, 760))
  const [seenChangeCounts, setSeenChangeCounts] = useState<Record<string, number>>(readStoredSeenChanges)
  const [dark, setDark] = useState(() => typeof window !== 'undefined' && window.localStorage.getItem(layoutStorage.theme) === 'dark')
  const [compactLayout, setCompactLayout] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 1180px)').matches)
  const [compactRightOpen, setCompactRightOpen] = useState(false)
  const [emptyConversation, setEmptyConversation] = useState(false)
  const [composerHeight, setComposerHeight] = useState(180)
  const [knowledgeDialogOpen, setKnowledgeDialogOpen] = useState(false)
  const [knowledgeDialogBusy, setKnowledgeDialogBusy] = useState(false)
  const [knowledgeDialogError, setKnowledgeDialogError] = useState<string | null>(null)
  const [localKnowledgeBases, setLocalKnowledgeBases] = useState<LocalKnowledgeBaseDto[]>([])
  const [localKnowledgeLoading, setLocalKnowledgeLoading] = useState(false)
  const [conversationDialog, setConversationDialog] = useState<{ conversation: { id: string; title: string }; mode: 'rename' | 'purge' } | null>(null)
  const [conversationDialogBusy, setConversationDialogBusy] = useState(false)
  const [conversationDialogError, setConversationDialogError] = useState<string | null>(null)
  const [projectDeleteDialog, setProjectDeleteDialog] = useState<{ name: string; root: string; items: Array<{ id: string; title: string }> } | null>(null)
  const [projectDeleteBusy, setProjectDeleteBusy] = useState(false)
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [chatState, setChatState] = useState<ChatState>('complete')
  const [activePrompt, setActivePrompt] = useState('Kun 的协议是什么协议，我可以拿来二次开发并且企业内部使用吗？')
  const [expertPickerOpen, setExpertPickerOpen] = useState(false)
  const [composerResetKey, setComposerResetKey] = useState(0)
  const [composerDraft, setComposerDraft] = useState('')
  const [pendingUserMessage, setPendingUserMessage] = useState<ConversationMessage | null>(null)
  useEffect(() => {
    const handleFork = (event: Event) => {
      const messageId = (event as CustomEvent<{ messageId?: string }>).detail?.messageId
      if (!messageId) return
      void desktopConversation.forkConversation(messageId).then((fork) => {
        if (!fork) {
          toast.error(desktopConversation.error ?? '无法创建会话分支')
          return
        }
        setPendingUserMessage(null)
        setEmptyConversation(false)
        setChatState('complete')
        toast.success('已从此消息创建独立分支')
      })
    }
    window.addEventListener(CONVERSATION_FORK_EVENT, handleFork)
    return () => window.removeEventListener(CONVERSATION_FORK_EVENT, handleFork)
  }, [desktopConversation.error, desktopConversation.forkConversation])
  const workflowTimer = useRef<number | null>(null)
  const desktopUserMessage = desktopConversation.detail ? latestMessage(desktopConversation.detail.messages, 'user') : undefined
  const desktopAssistantMessage = desktopConversation.detail ? latestMessage(desktopConversation.detail.messages, 'assistant') : undefined
  const desktopCurrentRunAssistantMessage = desktopConversation.detail?.lastRun
    ? [...desktopConversation.detail.messages].reverse().find((message) => (
        message.role === 'assistant' && message.runId === desktopConversation.detail?.lastRun?.id
      ))
    : undefined
  const desktopRunning = desktopConversation.enabled && runIsActive(desktopConversation.detail)
  const desktopRunStatus = conversationRunState(desktopConversation.detail)
  const selectableAgents = chatSelectableAgents(agentResource.agents)
  const activeAgent = resolveActiveChatAgent(agentResource.agents, desktopConversation.selectedAgentId)
  const activeExpert = agentResource.agents.find((item) => item.id === desktopConversation.selectedExpertId) ?? null
  const expertToolAvailability = useMemo(() => activeExpert
    ? latestExpertToolAvailability(desktopConversation.detail?.runtimeEvents ?? [], activeExpert)
    : undefined, [activeExpert, desktopConversation.detail?.runtimeEvents])
  const activeProject = projects.find((item) => item.id === desktopConversation.detail?.conversation.projectId)
    ?? projects.find((item) => item.rootPath === desktopConversation.draftProjectRoot)
    ?? null
  const conversationUsage = useMemo(() => latestConversationContextUsage(desktopConversation.detail?.runtimeEvents ?? []), [desktopConversation.detail?.runtimeEvents])
  const runtimeQuestion = useMemo(() => pendingRuntimeQuestion(desktopConversation.detail), [desktopConversation.detail])
  const goalProgressData = useGoalProgress(desktopConversation.detail)
  const conversationAgentId = desktopConversation.detail?.conversation.agentId
  const timelineAssistantName = desktopConversation.detail?.conversation.agentName ?? activeAgent?.name ?? 'Fox 默认助手'
  useEffect(() => {
    if (!conversationAgentId || LOCAL_KNOWLEDGE_WORKSPACE_VIEWS.includes(activeView)) return
    const conversationAgent = agentResource.agents.find((item) => item.id === conversationAgentId)
    if (conversationAgent) setAssistantMode(conversationAgent.runtimeType === 'yuxi' ? 'knowledge' : 'assistant')
  }, [activeView, agentResource.agents, conversationAgentId])
  const timelineRunModel = desktopConversation.detail?.lastRun?.model
  const pendingRuntimeApproval = desktopConversation.detail?.approvals.find((approval) => approval.status === 'pending')
  const expertReadOnly = expertInteractionLocked({
    running: desktopRunning || chatState === 'running',
    approvalPending: Boolean(pendingRuntimeApproval) || chatState === 'approval',
    questionPending: Boolean(runtimeQuestion) || chatState === 'question',
  })
  const visiblePrompt = desktopConversation.enabled ? pendingUserMessage?.content ?? desktopUserMessage?.content ?? activePrompt : activePrompt
  const visibleReply = desktopConversation.enabled
    ? desktopConversation.streamingText || desktopCurrentRunAssistantMessage?.content || ''
    : undefined
  const activeRuntimeEvents = useMemo(() => {
    const runId = desktopConversation.detail?.lastRun?.id
    if (!runId) return []
    return desktopConversation.detail?.runtimeEvents.filter((event) => event.runId === runId) ?? []
  }, [desktopConversation.detail?.lastRun?.id, desktopConversation.detail?.runtimeEvents])
  const activeConversationId = desktopConversation.detail?.conversation.id ?? null

  // File tabs are scoped to the active conversation. The Files page itself can
  // stay open safely because clearing these tabs restores its project tree.
  useEffect(() => {
    setOpenFileTabs([])
    setActiveFileTabId(null)
    setActiveChildAgent(null)
    if (rightMode === 'files' && !openRightTabs.includes('files')) {
      setRightMode(openRightTabs.at(-1) ?? null)
    }
  }, [activeConversationId])
  const totalChangeCount = desktopConversation.detail?.toolCalls.filter((tool) => isChangeOrCommandTool(tool.toolName)).length ?? 0

  const refreshKnowledgeChoices = useCallback(() => {
    setKnowledgeDialogError(null)
    void knowledge.refresh()
    if (!desktopRuntimeAvailable) {
      setLocalKnowledgeBases([])
      setLocalKnowledgeLoading(false)
      return
    }
    setLocalKnowledgeLoading(true)
    void desktopClient.listLocalKnowledgeBases()
      .then(setLocalKnowledgeBases)
      .catch((cause) => {
        setLocalKnowledgeBases([])
        setKnowledgeDialogError(cause instanceof Error ? cause.message : String(cause))
      })
      .finally(() => setLocalKnowledgeLoading(false))
  }, [knowledge.refresh])


  useEffect(() => {
    document.documentElement.classList.toggle('dark', dark)
    window.localStorage.setItem(layoutStorage.theme, dark ? 'dark' : 'light')
  }, [dark])
  useEffect(() => { window.localStorage.setItem(layoutStorage.sidebarCollapsed, sidebarCollapsed ? '1' : '0') }, [sidebarCollapsed])
  useEffect(() => { window.localStorage.setItem(layoutStorage.sidebarWidth, String(Math.round(sidebarWidth))) }, [sidebarWidth])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightWidth, String(Math.round(rightWidth))) }, [rightWidth])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightMode, rightMode ?? 'closed') }, [rightMode])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightTabs, JSON.stringify(openRightTabs)) }, [openRightTabs])
  useEffect(() => { window.localStorage.setItem(layoutStorage.rightSidebarCollapsed, rightSidebarCollapsed ? '1' : '0') }, [rightSidebarCollapsed])
  useEffect(() => { window.localStorage.setItem(layoutStorage.seenChanges, JSON.stringify(seenChangeCounts)) }, [seenChangeCounts])
  useEffect(() => {
    if (!activeConversationId || rightMode !== 'changes' || rightSidebarCollapsed) return
    setSeenChangeCounts((current) => current[activeConversationId] === totalChangeCount ? current : { ...current, [activeConversationId]: totalChangeCount })
  }, [activeConversationId, rightMode, rightSidebarCollapsed, totalChangeCount])
  useEffect(() => {
    if (!desktopConversation.enabled) return
    void desktopClient.listProjects().then(setProjects).catch(() => undefined)
  }, [desktopConversation.enabled, desktopConversation.detail?.conversation.projectId])
  useEffect(() => {
    if (!knowledgeDialogOpen) return
    refreshKnowledgeChoices()
  }, [knowledgeDialogOpen, refreshKnowledgeChoices])
  useEffect(() => {
    if (!knowledgeDialogOpen || !desktopRuntimeAvailable
      || !localKnowledgeBases.some(item => ['queued', 'running'].includes(item.activeJobStatus ?? ''))) return
    let disposed = false
    const timer = window.setTimeout(() => {
      void desktopClient.listLocalKnowledgeBases().then(items => {
        if (!disposed) setLocalKnowledgeBases(items)
      }).catch(() => undefined)
    }, 1500)
    return () => { disposed = true; window.clearTimeout(timer) }
  }, [knowledgeDialogOpen, localKnowledgeBases])
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

  const shellStyle = useMemo(() => {
    const currentSidebarWidth = sidebarCollapsed || compactLayout ? 64 : sidebarWidth
    return {
      '--fox-sidebar-width': `${currentSidebarWidth}px`,
      '--fox-sidebar-center-offset': `${currentSidebarWidth / 2}px`,
      '--fox-composer-occlusion': `${composerHeight}px`,
    } as React.CSSProperties
  }, [compactLayout, composerHeight, sidebarCollapsed, sidebarWidth])

  const openRightSidebarHome = () => {
    setRightSidebarCollapsed(false)
    setRightPanelMaximized(false)
    setRightMode(null)
    if (compactLayout) setCompactRightOpen(true)
  }
  const collapseRightSidebar = () => {
    setRightSidebarCollapsed(true)
    setRightPanelMaximized(false)
    setCompactRightOpen(false)
  }
  const selectRightMode = (mode: RightMode) => {
    setRightSidebarCollapsed(false)
    setOpenRightTabs((current) => current.includes(mode) ? current : [...current, mode])
    if (mode === 'files') setActiveFileTabId(null)
    if (mode === 'agents') setActiveChildAgent(null)
    if (mode === 'changes' && activeConversationId) {
      setSeenChangeCounts((current) => current[activeConversationId] === totalChangeCount ? current : { ...current, [activeConversationId]: totalChangeCount })
    }
    setRightMode(mode)
    if (compactLayout) setCompactRightOpen(true)
  }
  const closeRightMode = (mode: RightMode) => {
    const closingIndex = openRightTabs.indexOf(mode)
    if (closingIndex < 0) return
    const nextTabs = openRightTabs.filter((item) => item !== mode)
    setOpenRightTabs(nextTabs)
    const closingActivePage = rightMode === mode && (mode !== 'files' || activeFileTabId === null)
    if (mode === 'files' && closingActivePage && openFileTabs.length > 0) {
      setActiveFileTabId(openFileTabs[0].id)
      return
    }
    if (mode === 'agents') setActiveChildAgent(null)
    if (closingActivePage) {
      setRightMode(nextTabs[Math.min(closingIndex, nextTabs.length - 1)] ?? null)
    }
  }
  const openFile = (path: string, artifactId: string | null = null) => {
    const tab = createOpenFileTab(path, artifactId)
    setOpenFileTabs((current) => {
      const existing = current.find((item) => item.id === tab.id)
      if (!existing) return [...current, tab]
      if (!artifactId || existing.artifactId === artifactId) return current
      return current.map((item) => item.id === tab.id ? { ...item, artifactId } : item)
    })
    setRightSidebarCollapsed(false)
    setRightMode('files')
    setActiveFileTabId(tab.id)
    if (compactLayout) setCompactRightOpen(true)
  }
  const activateFile = (tabId: string) => {
    if (!openFileTabs.some((tab) => tab.id === tabId)) return
    setRightSidebarCollapsed(false)
    setRightMode('files')
    setActiveFileTabId(tabId)
    if (compactLayout) setCompactRightOpen(true)
  }
  const closeFile = (tabId: string) => {
    const closingIndex = openFileTabs.findIndex((tab) => tab.id === tabId)
    if (closingIndex < 0) return
    const nextTabs = openFileTabs.filter((tab) => tab.id !== tabId)
    const nextActiveId = activeFileTabId === tabId
      ? nextTabs[Math.min(closingIndex, nextTabs.length - 1)]?.id ?? null
      : activeFileTabId
    setOpenFileTabs(nextTabs)
    setActiveFileTabId(nextActiveId)
    if (rightMode === 'files' && activeFileTabId === tabId && !nextActiveId && !openRightTabs.includes('files')) {
      setRightMode(openRightTabs.at(-1) ?? null)
    }
  }
  const openChildAgent = (run: ChildRunRecord, avatarIndex: number) => {
    setActiveChildAgent({ run, avatarIndex })
    setRightSidebarCollapsed(false)
    setRightMode('agents')
    if (compactLayout) setCompactRightOpen(true)
  }
  const openArtifact = (artifact: ArtifactRecord) => {
    const projectRoot = desktopConversation.detail?.conversation.projectRoot
    openFile(projectRoot ? artifactProjectPath(artifact, projectRoot, []) : artifact.storagePath, artifact.id)
  }
  const openEvidence = (evidence: TaskEvidenceRecord) => {
    const scrollToMessage = (messageId: string) => {
      const element = document.getElementById(`fox-turn-${messageId}`)
      if (!element) return false
      element.scrollIntoView({ behavior: 'smooth', block: 'center' })
      element.animate(
        [{ backgroundColor: 'transparent' }, { backgroundColor: 'rgba(191, 219, 254, 0.42)' }, { backgroundColor: 'transparent' }],
        { duration: 1200, easing: 'ease-out' },
      )
      return true
    }
    if (evidence.refKind === 'message' && scrollToMessage(evidence.refId)) return
    if (evidence.refKind === 'artifact') {
      const artifact = desktopConversation.detail?.artifacts.find((item) => item.id === evidence.refId)
      const message = artifact?.runId
        ? desktopConversation.detail?.messages.find((item) => item.role === 'assistant' && item.runId === artifact.runId)
        : null
      if (message && scrollToMessage(message.id)) return
    }
    if (evidence.refKind === 'source') {
      const knowledgeBaseId = typeof evidence.metadata.knowledgeBaseId === 'string' ? evidence.metadata.knowledgeBaseId : null
      const documentId = typeof evidence.metadata.documentId === 'string' ? evidence.metadata.documentId : evidence.refId
      if (knowledgeBaseId && typeof documentId === 'string' && documentId) {
        navigate('knowledge-detail', knowledgeBaseId, { documentId })
        return
      }
    }
    if (evidence.refKind === 'tool_call' || evidence.refKind === 'run_event') {
      selectRightMode(evidence.evidenceType === 'file_diff' ? 'changes' : 'todo')
      return
    }
    toast.info('该证据的原始对象已不可用')
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
  const retryLatestRun = () => {
    if (!desktopConversation.enabled) {
      finishWorkflow()
      return
    }
    const latestUser = [...(desktopConversation.detail?.messages ?? [])]
      .reverse()
      .find((message) => message.role === 'user')
    if (!latestUser) return
    clearWorkflowTimer()
    setActivePrompt(latestUser.content)
    setComposerDraft('')
    setPendingUserMessage(null)
    setChatState('running')
    void desktopConversation
      .rerunFromMessage(latestUser.id, latestUser.content, conversationModelOverride(activeAgent, desktopConversation.detail?.lastRun?.model, yuxiModels.models))
      .then((started) => {
        if (!started) setChatState('error')
      })
  }
  const resetConversation = (suggestion = '') => {
    clearWorkflowTimer()
    if (desktopConversation.enabled) {
      void desktopConversation.createConversation()
    }
    setChatState('complete')
    setPendingUserMessage(null)
    setEmptyConversation(!suggestion)
    setComposerDraft(suggestion)
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
      setComposerDraft('')
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
  const switchAssistantMode = (mode: AssistantMode, newConversation = false) => {
    setAssistantMode(mode)
    if (mode === 'knowledge') {
      navigate('local-knowledge-home')
      return
    }
    if (mode === 'assistant') {
      if (!newConversation) {
        navigate('chat')
        return
      }
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
  }
  const createProjectDraft = (requestedProjectRoot?: string) => {
    const projectRoot = requestedProjectRoot
      ?? desktopConversation.detail?.conversation.projectRoot
      ?? desktopConversation.draftProjectRoot
      ?? desktopConversation.conversations.find((conversation) => conversation.projectRoot)?.projectRoot
    const requestedProject = requestedProjectRoot
      ? projects.find((project) => project.rootPath.replace(/[\\/]+$/, '').toLocaleLowerCase() === requestedProjectRoot.replace(/[\\/]+$/, '').toLocaleLowerCase())
      : activeProject
    const permissionMode = requestedProject?.permissionMode ?? desktopConversation.draftPermissionMode ?? readDefaultProjectPermission()
    if (!projectRoot) {
      void addProject()
      return
    }
    const agentId = desktopConversation.selectedAgentId ?? (assistantMode === 'assistant' ? 'fox-general' : null)
    if (!agentId) {
      toast.error('请先选择一个专家')
      return
    }
    desktopConversation.selectProject(projectRoot, permissionMode)
    setActiveView('chat')
  }
  const pickProjectFolder = async () => {
    try {
      return validPickedProjectFolder(await desktopClient.pickProjectFolder())
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause)
      toast.error(message)
      return null
    }
  }
  const addProject = async () => {
    const selected = await pickProjectFolder()
    if (!selected) return
    const projectSelection = selectProjectRoot(selected, projects, readDefaultProjectPermission())
    desktopConversation.selectProject(projectSelection.path, projectSelection.permissionMode)
    setActiveView('chat')
    toast.success(`项目“${selected.split(/[\\/]/).filter(Boolean).at(-1) ?? selected}”已加入当前对话`)
  }
  const changeProjectPermission = async (permissionMode: ProjectRecord['permissionMode']) => {
    desktopConversation.setDraftPermission(permissionMode)
    const projectId = desktopConversation.detail?.conversation.projectId ?? activeProject?.id
    if (projectId) {
      setProjects((current) => current.map((project) => project.id === projectId ? { ...project, permissionMode } : project))
      try {
        const updated = await desktopClient.updateProjectPermission(projectId, permissionMode)
        setProjects((current) => current.some((project) => project.id === updated.id)
          ? current.map((project) => project.id === updated.id ? updated : project)
          : [...current, updated])
      } catch (cause) {
        void desktopClient.listProjects().then(setProjects).catch(() => undefined)
        toast.error(cause instanceof Error ? cause.message : String(cause))
        return
      }
    } else if (desktopConversation.detail?.conversation.id) {
      try {
        await desktopClient.updateConversationPermission(
          desktopConversation.detail.conversation.id,
          permissionMode,
        )
        await desktopConversation.openConversation(desktopConversation.detail.conversation.id)
      } catch (cause) {
        toast.error(cause instanceof Error ? cause.message : String(cause))
        return
      }
    }
  }
  const saveKnowledgeBindings = async (references: KnowledgeReference[], names: Record<string, string>, closeDialog = true) => {
    setKnowledgeDialogBusy(true)
    setKnowledgeDialogError(null)
    try {
      const saved = await desktopConversation.setKnowledgeReferences(references, names)
      if (!saved) throw new Error(desktopConversation.error ?? '无法保存知识库选择')
    } catch (cause) {
      setKnowledgeDialogBusy(false)
      setKnowledgeDialogError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
    setKnowledgeDialogBusy(false)
    if (closeDialog) setKnowledgeDialogOpen(false)
    toast.success(references.length ? `已为当前会话启用 ${references.length} 个知识库` : '已清除当前会话的知识库')
    return true
  }
  const toggleKnowledgeBinding = async (reference: KnowledgeReference, name: string) => {
    const currentReferences = Array.isArray(desktopConversation.knowledgeReferences)
      ? desktopConversation.knowledgeReferences
      : desktopConversation.knowledgeBindings.filter((binding) => binding.enabled !== false).map(knowledgeReferenceFromLegacyBinding)
    const targetKey = knowledgeReferenceKey(reference)
    const nextReferences = currentReferences.some((item) => knowledgeReferenceKey(item) === targetKey)
      ? currentReferences.filter((item) => knowledgeReferenceKey(item) !== targetKey)
      : [...currentReferences, reference]
    const names = Object.fromEntries(nextReferences.map((item) => {
      const key = knowledgeReferenceKey(item)
      const remoteName = knowledge.items.find((candidate) => candidate.id === item.id)?.name
      const localName = localKnowledgeBases.find((candidate) => candidate.id === item.id)?.name
      const legacyName = desktopConversation.knowledgeBindings.find((binding) => binding.knowledgeBaseId === item.id)?.knowledgeBaseName
      return [key, key === targetKey ? name : remoteName ?? localName ?? legacyName ?? item.id]
    }))
    return saveKnowledgeBindings(nextReferences, names, false)
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
  const pinManagedConversation = async (conversation: ConversationSummary) => {
    const pinned = !conversation.pinned
    if (!await desktopConversation.setConversationPinned(conversation.id, pinned)) {
      toast.error(desktopConversation.error ?? '无法更新对话置顶状态')
      return
    }
    toast.success(pinned ? '对话已置顶' : '已取消置顶')
  }
  const archiveManagedConversation = async (conversation: ConversationSummary) => {
    if (!await desktopConversation.archiveConversation(conversation.id)) {
      toast.error(desktopConversation.error ?? '无法归档对话')
      return
    }
    setPendingUserMessage(null)
    setEmptyConversation(true)
    toast.success('对话已归档')
  }
  const unarchiveManagedConversation = async (conversation: ConversationSummary) => {
    if (!await desktopConversation.unarchiveConversation(conversation.id)) {
      toast.error(desktopConversation.error ?? '无法恢复归档对话')
      return
    }
    toast.success('对话已恢复')
  }
  const trashManagedConversation = async (conversation: ConversationSummary) => {
    const result = await desktopConversation.deleteConversation(conversation.id)
    if (!result.deleted) {
      toast.error(result.error ?? '无法移入回收站')
      return
    }
    if (desktopConversation.detail?.conversation.id === conversation.id) {
      setPendingUserMessage(null)
      setEmptyConversation(true)
    }
    toast.success('对话已移入回收站')
  }
  const restoreManagedConversation = async (conversation: ConversationSummary) => {
    if (!await desktopConversation.restoreConversation(conversation.id)) {
      toast.error(desktopConversation.error ?? '无法恢复对话')
      return
    }
    toast.success('对话已恢复')
  }
  const purgeManagedConversation = async () => {
    if (!conversationDialog) return
    setConversationDialogBusy(true)
    setConversationDialogError(null)
    const result = await desktopConversation.purgeConversation(conversationDialog.conversation.id)
    setConversationDialogBusy(false)
    if (!result.deleted) {
      setConversationDialogError(result.error ?? '无法永久删除对话，请稍后重试')
      return
    }
    setConversationDialog(null)
    setConversationDialogError(null)
    toast.success('对话已永久删除')
  }
  const deleteManagedProject = async () => {
    if (!projectDeleteDialog) return
    setProjectDeleteBusy(true)
    for (const conversation of projectDeleteDialog.items) {
      const result = await desktopConversation.deleteConversation(conversation.id)
      if (!result.deleted) {
        setProjectDeleteBusy(false)
        toast.error(result.error ?? '项目删除未完成，请先停止正在运行的对话后重试')
        return
      }
    }
    const normalizedRoot = projectDeleteDialog.root.replace(/[\\/]+$/, '').replace(/\//g, '\\').toLocaleLowerCase()
    const projectRecord = projects.find((project) => project.rootPath.replace(/[\\/]+$/, '').replace(/\//g, '\\').toLocaleLowerCase() === normalizedRoot)
    if (projectRecord) {
      try {
        await desktopClient.deleteProject(projectRecord.id)
        setProjects((current) => current.filter((project) => project.id !== projectRecord.id))
      } catch (cause) {
        setProjectDeleteBusy(false)
        toast.error(cause instanceof Error ? cause.message : String(cause))
        return
      }
    }
    setProjectDeleteBusy(false)
    setProjectDeleteDialog(null)
    toast.success('项目已从 Fox 删除，本机文件夹未受影响')
  }
  const selectExpert = useStableCallback(async (expertId: string): Promise<boolean> => {
    if (expertReadOnly) return false
    if (expertId === desktopConversation.selectedExpertId) return true
    const targetAgent = agentResource.agents.find((agent) => agent.id === expertId)
    if (!targetAgent) return false
    let configuredKnowledgeReferences: KnowledgeReference[] = []
    try {
      const declaration = resolveExpertKnowledgeDeclaration(targetAgent.packageManifest)
      if (declaration.format === 'references') {
        configuredKnowledgeReferences = declaration.references
      } else if (declaration.format === 'legacy_remote_ids') {
        configuredKnowledgeReferences = declaration.ids.map((id) => ({
          source: 'remote',
          connectionId: 'yuxi',
          id,
        }))
      }
    } catch (cause) {
      toast.error('专家知识库配置无效', { description: cause instanceof Error ? cause.message : String(cause) })
      return false
    }
    const result = await desktopConversation.createConversationForExpert(targetAgent.id)
    if (!result.success) {
      toast.error(expertErrorMessage(result.error, '无法召唤这个专家'))
      return false
    }
    if (configuredKnowledgeReferences.length) {
      const references = [...(desktopConversation.knowledgeReferences ?? desktopConversation.knowledgeBindings.map(knowledgeReferenceFromLegacyBinding))]
      for (const reference of configuredKnowledgeReferences) {
        if (!references.some((item) => knowledgeReferenceKey(item) === knowledgeReferenceKey(reference))) references.push(reference)
      }
      const names = Object.fromEntries(references.map((reference) => [
        knowledgeReferenceKey(reference),
        desktopConversation.knowledgeReferenceNames[knowledgeReferenceKey(reference)] ?? (reference.source === 'local' ? localKnowledgeBases : knowledge.items).find((item) => item.id === reference.id)?.name ?? reference.id,
      ]))
      if (!await desktopConversation.setKnowledgeReferences(references, names)) {
        toast.error('专家已启用，但知识库绑定失败')
      }
    }
    return true
  })
  const navigate: NavigateWorkspace = (view, entityId, context) => {
    if (view === 'onboarding' && activeView !== 'onboarding') {
      onboardingReturnRoute.current = { view: activeView, entityId: activeEntityId, documentId: activeDocumentId }
    }
    if (view === 'chat' && entityId) {
      const targetAgent = agentResource.agents.find((item) => item.id === entityId)
      if (!targetAgent) return
      const targetKind = normalizeAgentClassification(targetAgent).agentKind
      if (targetKind === 'worker') return
      if (targetKind === 'expert') void selectExpert(entityId)
      if (targetKind === 'assistant' && entityId !== desktopConversation.detail?.conversation.agentId) {
        void desktopConversation.createConversationForAgent(entityId).then((created) => {
          if (!created) toast.error(desktopConversation.error ?? '无法切换这个助手')
        })
      }
    }
    managementReturnRoutes.current = captureManagementReturnRoutes(
      managementReturnRoutes.current,
      { view: activeView, entityId: activeEntityId, documentId: activeDocumentId },
      view,
    )
    setActiveEntityId(entityId ?? null)
    setActiveDocumentId(context?.documentId ?? null)
    setActiveSourceLocator(context?.sourceLocator ?? null)
    setActiveView(view)
    setCompactRightOpen(false)
    setRightSidebarCollapsed(true)
    setRightPanelMaximized(false)
    setRightMode(null)
  }
  const requestLocalKnowledgeCreate = () => {
    setLocalKnowledgeCreateRequest((value) => value + 1)
    navigate('local-knowledge')
  }
  useEffect(() => {
    if (!LOCAL_KNOWLEDGE_WORKSPACE_VIEWS.includes(activeView)) return
    setAssistantMode('knowledge')
  }, [activeView])
  const exitManagement = () => {
    const previous = managementExitRoute(activeView, managementReturnRoutes.current)
    setActiveView(previous.view)
    setActiveEntityId(previous.entityId)
    setActiveDocumentId(previous.documentId)
    setActiveSourceLocator(null)
    setCompactRightOpen(false)
    setRightSidebarCollapsed(true)
    setRightPanelMaximized(false)
    setRightMode(null)
  }
  const finishOnboarding = (status: 'completed' | 'skipped') => {
    window.localStorage.setItem('fox.onboarding.status', status)
    const target = onboardingReturnRoute.current
    setActiveView(target.view === 'onboarding' ? 'chat' : target.view)
    setActiveEntityId(target.entityId)
    setActiveDocumentId(target.documentId)
    setActiveSourceLocator(null)
    setCompactRightOpen(false)
    setRightSidebarCollapsed(true)
    setRightPanelMaximized(false)
    setRightMode(null)
    if (status === 'skipped' && !modelService.service) toast('设置已跳过，配置模型服务后才能正常使用 Fox 助手')
  }
  useEffect(() => {
    const handleShortcut = (event: globalThis.KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey) return
      const key = event.key.toLocaleLowerCase()
      if (!['n', 'o', ',', 'b'].includes(key)) return
      event.preventDefault()
      if (key === 'n') switchAssistantMode(assistantMode)
      if (key === 'o') void addProject()
      if (key === ',') navigate('settings')
      if (key === 'b') setSidebarCollapsed((value) => !value)
    }
    window.addEventListener('keydown', handleShortcut)
    return () => window.removeEventListener('keydown', handleShortcut)
  })

  const workspacePage = activeView === 'chat' || activeView === 'onboarding' ? null : (
    <WorkspaceShell
      activeView={activeView}
      sidebarCollapsed={sidebarCollapsed || compactLayout}
      onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)}
      navigate={navigate}
      activeEntityId={activeEntityId}
      activeDocumentId={activeDocumentId}
      activeSourceLocator={activeSourceLocator}
      dark={dark}
      onDark={() => setDark(!dark)}
      onAskKnowledge={askKnowledgeBase}
      localKnowledgeCreateRequest={localKnowledgeCreateRequest}
      onLocalKnowledgeCreateRequestHandled={() => setLocalKnowledgeCreateRequest(0)}
    />
  )
  const timelineEmpty = emptyConversation || (desktopConversation.enabled ? !desktopUserMessage && !pendingUserMessage : false)
  useEffect(() => {
    if (activeView !== 'chat' || !timelineEmpty) return
    setRightSidebarCollapsed(true)
    setCompactRightOpen(false)
  }, [activeView, timelineEmpty])
  const showConversationRightSidebar = activeView === 'chat' && !timelineEmpty
  const handleRuntimeOpenSource = useStableCallback((source: RuntimeSource) => {
    navigate('knowledge-detail', source.knowledgeBaseId, { documentId: source.documentId, sourceLocator: source.locator })
  })
  const handleRuntimeSubmitQuestion = useStableCallback(async (text: string, answers: Record<string, string | string[]>) => {
    if (!runtimeQuestion) return false
    clearWorkflowTimer()
    setActivePrompt(text)
    setChatState('running')
    const started = await desktopConversation.resumeQuestion(runtimeQuestion.runId, text, answers)
    if (!started) {
      setChatState('error')
      return false
    }
    return true
  })
  const handleKnowledgeOpen = useStableCallback(() => {
    setKnowledgeDialogOpen(true)
    refreshKnowledgeChoices()
  })
  const handleKnowledgeToggle = useStableCallback((reference: KnowledgeReference, name: string) => toggleKnowledgeBinding(reference, name))
  const recoveryConversationId = desktopConversation.detail?.conversation.id
  const recoveryRunId = desktopConversation.enabled && desktopConversation.detail?.kernelSnapshot?.terminalWritten
    && desktopConversation.detail.kernelSnapshot.runId === desktopConversation.detail.lastRun?.id
    ? desktopConversation.detail.lastRun?.id : undefined
  const handleRecoveryResumed = useStableCallback(async () => {
    if (recoveryConversationId) await desktopConversation.openConversation(recoveryConversationId)
  })
  const recoveryPanel = useMemo(() => recoveryConversationId && recoveryRunId
    ? <KernelReconciliationPanel key={recoveryRunId} conversationId={recoveryConversationId} runId={recoveryRunId} onResumed={handleRecoveryResumed} />
    : null, [recoveryConversationId, recoveryRunId, handleRecoveryResumed])
  const timelineRuntimeContextValue = useMemo<TimelineRuntimeContextValue>(() => ({
    knowledgeBindings: desktopConversation.knowledgeBindings,
    assistantName: timelineAssistantName,
    runModel: timelineRunModel,
    onOpenSource: handleRuntimeOpenSource,
    recoveryPanel,
    userProfile,
  }), [desktopConversation.knowledgeBindings, handleRuntimeOpenSource, timelineAssistantName, timelineRunModel, recoveryPanel, userProfile.name, userProfile.avatar, userProfile.initial])
  const composerRuntimeContextValue = useMemo<ComposerRuntimeContextValue>(() => ({
    ...timelineRuntimeContextValue,
    remoteKnowledgeBases: knowledge.items,
    localKnowledgeBases,
    knowledgeReferences: desktopConversation.knowledgeReferences,
    knowledgeReferenceNames: desktopConversation.knowledgeReferenceNames,
    knowledgeLoading: knowledge.loading || localKnowledgeLoading || knowledgeDialogBusy,
    knowledgeError: knowledgeDialogError ?? knowledge.error,
    onKnowledgeMenuOpen: refreshKnowledgeChoices,
    onKnowledgeToggle: handleKnowledgeToggle,
    questionRequest: runtimeQuestion,
    onSubmitQuestion: handleRuntimeSubmitQuestion,
    onKnowledge: handleKnowledgeOpen,
  }), [desktopConversation.knowledgeReferences, desktopConversation.knowledgeReferenceNames, handleKnowledgeOpen, handleKnowledgeToggle, handleRuntimeSubmitQuestion, knowledge.error, knowledge.items, knowledge.loading, knowledgeDialogBusy, knowledgeDialogError, localKnowledgeBases, localKnowledgeLoading, refreshKnowledgeChoices, runtimeQuestion, timelineRuntimeContextValue])
  const handleTimelineLoadEarlierMessages = useStableCallback(() => {
    void desktopConversation.loadEarlierMessages()
  })
  const handleTimelineApprove = useStableCallback(() => finishWorkflow())
  const handleTimelineDeny = useStableCallback(() => {
    clearWorkflowTimer()
    setChatState('denied')
  })
  const handleTimelineRetry = useStableCallback(() => {
    void retryLatestRun()
  })
  const handleTimelineRerun = useStableCallback(async (messageId: string, prompt: string) => {
    clearWorkflowTimer()
    setActivePrompt(prompt)
    setComposerDraft('')
    setPendingUserMessage(null)
    setEmptyConversation(false)
    setChatState('running')
    if (!desktopConversation.enabled) {
      finishWorkflow()
      return true
    }
    const started = await desktopConversation.rerunFromMessage(messageId, prompt, conversationModelOverride(activeAgent, desktopConversation.detail?.lastRun?.model, yuxiModels.models))
    if (!started) setChatState('error')
    return started
  })
  const handleTimelineAnswer = useStableCallback((_answer: string) => {
    finishWorkflow(2600)
  })
  const handleTimelineStart = useStableCallback((suggestion: string) => resetConversation(suggestion))
  const handleTimelineOpenArtifact = useStableCallback((artifact: ArtifactRecord) => {
    void openArtifact(artifact)
  })
  const handleTimelineViewExpert = useStableCallback((expertId: string) => navigate('agent-detail', expertId))
  const handleComposerProject = useStableCallback(() => {
    void addProject()
  })
  const handleComposerPermissionModeChange = useStableCallback((permissionMode: ProjectRecord['permissionMode']) => changeProjectPermission(permissionMode))
  const handleComposerAgentChange = useStableCallback((agentId: string) => {
    void desktopConversation.createConversationForAgent(agentId).then((created) => {
      if (!created) toast.error(desktopConversation.error ?? '无法切换这个助手')
    })
  })
  const handleComposerViewExpert = useStableCallback(() => {
    if (activeExpert) navigate('agent-detail', activeExpert.id)
  })
  const handleComposerChangeExpert = useStableCallback(() => setExpertPickerOpen(true))
  const handleComposerRemoveExpert = useStableCallback(async () => {
    const result = await desktopConversation.removeExpert()
    if (!result.success) {
      toast.error(expertErrorMessage(result.error, '无法移除专家，请稍后重试'))
      return
    }
    toast.success('已移除专家')
  })
  const handleComposerPromptCommit = useStableCallback((prompt: string) => {
    const now = Date.now()
    setActivePrompt(prompt)
    setComposerDraft('')
    setEmptyConversation(false)
    setChatState('running')
    setPendingUserMessage({
      id: `ui-pending-${now}`,
      conversationId: desktopConversation.detail?.conversation.id ?? 'pending',
      runId: null,
      role: 'user',
      kind: 'text',
      content: prompt,
      status: 'sending',
      ordinal: (desktopConversation.detail?.messages.at(-1)?.ordinal ?? 0) + 1,
      createdAt: now,
      updatedAt: now,
    })
  })
  const handleComposerSubmitPrompt = useStableCallback(async (prompt: string, model?: string, submittedFiles?: Array<{ filename?: string; mediaType?: string; url?: string }>, runtimeText?: string) => {
    clearWorkflowTimer()
    setActivePrompt(prompt)
    setComposerDraft('')
    setEmptyConversation(false)
    if (desktopConversation.enabled) {
      setChatState('running')
      const started = await desktopConversation.send(prompt, conversationModelOverride(activeAgent, model, yuxiModels.models), submittedFiles, runtimeText)
      if (!started) {
        setPendingUserMessage((message) => message ? { ...message, status: 'failed', updatedAt: Date.now() } : message)
        setChatState('error')
        return 'error' as const
      }
      return 'complete' as const
    }
    if (/询问我|让我选择|需要确认方案|怎么处理/.test(prompt)) {
      setChatState('question')
      return 'question' as const
    }
    if (/修改|写入|删除|重命名|创建文件/.test(prompt)) {
      setChatState('approval')
      return 'approval' as const
    }
    if (/失败|错误|连接测试|检查连接/.test(prompt)) {
      setChatState('error')
      return 'error' as const
    }
    setChatState('running')
    return 'complete' as const
  })
  const handleComposerResolveApproval = useStableCallback((approvalId: string, decision: ApprovalDecision) => desktopConversation.resolveApproval(approvalId, decision))
  const handleComposerResolveWorkModeConfirmation = useStableCallback((goalId: string, expectedVersion: number, approved: boolean) => desktopConversation.resolveWorkModeConfirmation(goalId, expectedVersion, approved))
  const handleComposerResolvePlanRevision = useStableCallback((planRevisionId: string, decision: 'approved' | 'rejected') => desktopConversation.resolvePlanRevision(planRevisionId, decision))
  const handleComposerDeleteGoal = useStableCallback((goalId: string) => desktopConversation.deleteGoal(goalId))
  const handleComposerGoalRunningChange = useStableCallback((goalId: string, expectedVersion: number, running: boolean) => desktopConversation.setGoalRunning(goalId, expectedVersion, running))
  const handleComposerEvidenceClick = useStableCallback((evidence: TaskEvidenceRecord) => openEvidence(evidence))
  const handleComposerStatusChange = useStableCallback((status: 'ready' | 'streaming') => {
    if (!desktopConversation.enabled && status === 'ready' && chatState === 'running') setChatState('complete')
  })
  const handleComposerCancel = useStableCallback(() => desktopConversation.cancel())
  const handleWorkspaceNavigate = useStableCallback((...args: Parameters<NavigateWorkspace>) => navigate(...args))
  const handleAssistantModeChange = useStableCallback((mode: AssistantMode) => switchAssistantMode(mode))
  const handleNewChat = useStableCallback(() => switchAssistantMode('assistant', true))
  const handleNewProjectChat = useStableCallback((projectRoot?: string) => createProjectDraft(projectRoot))
  const handleCreateLocalKnowledge = useStableCallback(() => requestLocalKnowledgeCreate())
  const handleOpenConversation = useStableCallback((conversationId?: string) => {
    setPendingUserMessage(null)
    if (desktopConversation.enabled && conversationId) void desktopConversation.openConversation(conversationId)
    setEmptyConversation(false)
    navigate('chat')
  })
  const handleRenameConversation = useStableCallback((conversation: ConversationSummary) => setConversationDialog({ conversation, mode: 'rename' }))
  const handlePinConversation = useStableCallback((conversation: ConversationSummary) => {
    void pinManagedConversation(conversation)
  })
  const handleArchiveConversation = useStableCallback((conversation: ConversationSummary) => {
    void archiveManagedConversation(conversation)
  })
  const handleUnarchiveConversation = useStableCallback((conversation: ConversationSummary) => {
    void unarchiveManagedConversation(conversation)
  })
  const handleTrashConversation = useStableCallback((conversation: ConversationSummary) => {
    void trashManagedConversation(conversation)
  })
  const handleRestoreConversation = useStableCallback((conversation: ConversationSummary) => {
    void restoreManagedConversation(conversation)
  })
  const handlePurgeConversation = useStableCallback((conversation: ConversationSummary) => setConversationDialog({ conversation, mode: 'purge' }))
  const handleDeleteProject = useStableCallback((project: { name: string; root: string; items: Array<{ id: string; title: string }> }) => {
    const normalizedRoot = project.root.replace(/[\\/]+$/, '').replace(/\//g, '\\').toLocaleLowerCase()
    setProjectDeleteDialog({
      ...project,
      items: desktopConversation.conversations
        .filter((conversation) => conversation.projectRoot?.replace(/[\\/]+$/, '').replace(/\//g, '\\').toLocaleLowerCase() === normalizedRoot)
        .map((conversation) => ({ id: conversation.id, title: conversation.title })),
    })
  })
  const handleExitManagement = useStableCallback(() => exitManagement())
  const handleExpandSidebar = useStableCallback(() => setSidebarCollapsed(false))
  const handleToggleTheme = useStableCallback(() => setDark(!dark))
  const handleOpenSettings = useStableCallback(() => navigate('settings'))
  const handleOpenAbout = useStableCallback(() => navigate('settings-about'))
  const handleToggleSidebar = useStableCallback(() => setSidebarCollapsed((value) => !value))
  const handleZoom = useStableCallback((delta: number) => {
    const current = Number(document.documentElement.dataset.foxZoom ?? '1')
    const next = Math.min(1.4, Math.max(.8, Math.round((current + delta) * 10) / 10))
    document.documentElement.dataset.foxZoom = String(next)
    document.documentElement.style.zoom = String(next)
  })
  const sidebarYuxiService = useMemo(() => yuxi.service ? {
    name: yuxi.service.name,
    status: yuxi.service.lastStatus,
    connectionType: yuxi.service.connectionType,
  } : null, [yuxi.service])

  if (activeView === 'onboarding') return <Suspense fallback={<div className="fox-app-loading"><img src={FOX_ASSISTANT_AVATAR} alt="" /><p>Fox 正在准备引导页</p></div>}><OnboardingPage navigate={navigate} onExit={finishOnboarding} /></Suspense>

  return (
    <main className="fox-shell" style={shellStyle}>
      <MemoizedWindowTitlebar leftSidebarCollapsed={sidebarCollapsed || compactLayout} onNewChat={handleNewChat} onOpenProject={handleComposerProject} onSettings={handleOpenSettings} onAbout={handleOpenAbout} onToggleSidebar={handleToggleSidebar} onZoom={handleZoom} />
      <div className="fox-workbench">
        <MemoizedSidebar collapsed={sidebarCollapsed || compactLayout} assistantMode={assistantMode} onModeChange={handleAssistantModeChange} onNewChat={handleNewChat} onNewProjectChat={handleNewProjectChat} onAddProject={handleComposerProject} onCreateLocalKnowledge={handleCreateLocalKnowledge} onOpenConversation={handleOpenConversation} onRenameConversation={handleRenameConversation} onPinConversation={handlePinConversation} onArchiveConversation={handleArchiveConversation} onUnarchiveConversation={handleUnarchiveConversation} onTrashConversation={handleTrashConversation} onRestoreConversation={handleRestoreConversation} onPurgeConversation={handlePurgeConversation} onDeleteProject={handleDeleteProject} onNavigate={handleWorkspaceNavigate} onExitManagement={handleExitManagement} onExpand={handleExpandSidebar} onTheme={handleToggleTheme} onSettings={handleOpenSettings} runtimeConversations={desktopConversation.enabled ? desktopConversation.conversations : undefined} archivedConversations={desktopConversation.enabled ? desktopConversation.archivedConversations : undefined} trashedConversations={desktopConversation.enabled ? desktopConversation.trashedConversations : undefined} onGlobalSearch={desktopRuntimeAvailable ? desktopClient.globalSearch : undefined} activeConversationId={desktopConversation.detail?.conversation.id} newChatActive={activeView === 'chat' && timelineEmpty} yuxiService={sidebarYuxiService} yuxiUser={yuxiUser.user} activeView={activeView} activeEntityId={activeEntityId} activeDocumentId={activeDocumentId} />
        {!sidebarCollapsed && !compactLayout && <SidebarResizeDivider onResize={(delta) => setSidebarWidth((value) => Math.min(456, Math.max(220, value + delta)))} />}
        <div className="fox-content-card">
        <div className={`fox-content-surface ${rightPanelMaximized && showConversationRightSidebar && !rightSidebarCollapsed ? 'is-right-maximized' : ''}`}>
        <section className="fox-chat-pane">
          <ExpertPickerDialog open={expertPickerOpen} onOpenChange={setExpertPickerOpen} agents={agentResource.agents} selectedExpertId={desktopConversation.selectedExpertId} readOnly={expertReadOnly} onSelect={selectExpert} />
          <TimelineRuntimeContext.Provider value={timelineRuntimeContextValue}>
          <ComposerRuntimeContext.Provider value={composerRuntimeContextValue}>
          {workspacePage ?? <>{!timelineEmpty && <ChatTopbar rightSidebarCollapsed={rightSidebarCollapsed} onRightSidebarExpand={openRightSidebarHome} onOpenRightMode={selectRightMode} conversation={desktopConversation.detail?.conversation} detail={desktopConversation.detail} modelName={timelineRunModel || modelService.service?.modelId || (activeAgent?.defaultModel !== 'configured-model' ? activeAgent?.defaultModel : undefined) || '未配置模型'} usage={conversationUsage} contextWindow={modelService.service?.contextWindow ?? 0} state={chatState} onPinConversation={(conversation) => void pinManagedConversation(conversation)} onRenameConversation={(conversation) => setConversationDialog({ conversation, mode: 'rename' })} onArchiveConversation={(conversation) => void archiveManagedConversation(conversation)} />}
<div className={`fox-chat-stage ${timelineEmpty ? 'is-empty' : ''}`}><Profiler id="conversation-timeline" onRender={recordRegionRender}><MemoizedTimeline hasEarlierMessages={desktopConversation.detail?.hasEarlierMessages} loadingEarlierMessages={desktopConversation.loadingEarlierMessages} onLoadEarlierMessages={handleTimelineLoadEarlierMessages} empty={timelineEmpty} state={chatState} prompt={visiblePrompt} openingSuggestions={activeAgent?.openingSuggestions} runtimeMessages={desktopConversation.enabled ? desktopConversation.detail?.messages ?? EMPTY_CONVERSATION_MESSAGES : undefined} runtimeAttachments={desktopConversation.enabled ? desktopConversation.detail?.attachments ?? EMPTY_CONVERSATION_ATTACHMENTS : undefined} runtimeArtifacts={desktopConversation.enabled ? desktopConversation.detail?.artifacts ?? EMPTY_RUNTIME_ARTIFACTS : undefined} expertBindings={desktopConversation.expertBindings} agents={agentResource.agents} pendingMessage={pendingUserMessage} runtimeEvents={desktopConversation.enabled ? desktopConversation.detail?.runtimeEvents : undefined} runtimeRunId={desktopConversation.detail?.lastRun?.id} runtimeRunning={desktopRunning} runtimeReply={visibleReply} runtimeError={desktopConversation.error} runtimeErrorDetails={desktopConversation.errorDetails} planRevisions={goalProgressData?.planRevisions} onApprove={handleTimelineApprove} onDeny={handleTimelineDeny} onRetry={handleTimelineRetry} onRerun={handleTimelineRerun} onAnswer={handleTimelineAnswer} onStart={handleTimelineStart} onOpenArtifact={handleTimelineOpenArtifact} onViewExpert={handleTimelineViewExpert} /></Profiler><Profiler id="conversation-composer" onRender={recordRegionRender}><MemoizedComposer resetKey={composerResetKey} draft={composerDraft} setDraft={setComposerDraft} chatState={chatState} centered={timelineEmpty} runtimeControlled={desktopConversation.enabled} runtimeInitializing={desktopConversation.enabled && !desktopConversation.ready && !desktopConversation.error} projectRoot={desktopConversation.detail?.conversation.projectRoot ?? desktopConversation.draftProjectRoot} projectPermissionMode={activeProject?.permissionMode ?? desktopConversation.detail?.conversation.permissionMode ?? desktopConversation.draftPermissionMode} activeAgent={activeAgent} activeExpert={activeExpert} expertReadOnly={expertReadOnly} expertToolAvailability={expertToolAvailability} agents={agentResource.agents} modelService={modelService.service} runtimeCapabilities={desktopConversation.runtimeStatus?.capabilities} yuxiModels={yuxiModels.models} usage={conversationUsage} goalProgressData={goalProgressData} runtimeApprovals={desktopConversation.enabled ? desktopConversation.detail?.approvals ?? EMPTY_RUNTIME_APPROVALS : EMPTY_RUNTIME_APPROVALS} onProject={handleComposerProject} onPermissionModeChange={handleComposerPermissionModeChange} onAgentChange={handleComposerAgentChange} onViewExpert={handleComposerViewExpert} onChangeExpert={handleComposerChangeExpert} onRemoveExpert={handleComposerRemoveExpert} onHeightChange={setComposerHeight} onPromptCommit={handleComposerPromptCommit} onSubmitPrompt={handleComposerSubmitPrompt} onApprove={handleTimelineApprove} onDeny={handleTimelineDeny} onAnswer={handleTimelineAnswer} onResolveApproval={handleComposerResolveApproval} onResolveWorkModeConfirmation={handleComposerResolveWorkModeConfirmation} onResolvePlanRevision={handleComposerResolvePlanRevision} onDeleteGoal={handleComposerDeleteGoal} onGoalRunningChange={handleComposerGoalRunningChange} onEvidenceClick={handleComposerEvidenceClick} onStatusChange={handleComposerStatusChange} onCancel={desktopConversation.enabled ? handleComposerCancel : undefined} /></Profiler></div></>}
          </ComposerRuntimeContext.Provider>
          </TimelineRuntimeContext.Provider>
        </section>
        {showConversationRightSidebar && !rightSidebarCollapsed && !compactLayout && <>{!rightPanelMaximized && <ResizeDivider onResize={(delta) => setRightWidth((value) => Math.min(760, Math.max(280, value + delta)))} />}<RightPanel mode={rightMode} tabs={openRightTabs} detail={desktopConversation.detail} width={rightWidth} maximized={rightPanelMaximized} fileTabs={openFileTabs} activeFileTabId={activeFileTabId} activeChildAgent={activeChildAgent} onMode={selectRightMode} onCloseMode={closeRightMode} onOpenFile={openFile} onActivateFile={activateFile} onCloseFile={closeFile} onOpenChildAgent={openChildAgent} onBackChildAgent={() => setActiveChildAgent(null)} onOpenEvidence={openEvidence} onToggleMaximized={() => setRightPanelMaximized((value) => !value)} onCollapse={collapseRightSidebar} /></>}
        </div>
        </div>
      </div>
      <Sheet open={showConversationRightSidebar && !rightSidebarCollapsed && compactLayout && compactRightOpen} onOpenChange={(open) => { if (open) setCompactRightOpen(true); else collapseRightSidebar() }}>
        <SheetContent side="right" showCloseButton={false} className={`fox-compact-context-sheet ${rightPanelMaximized ? 'is-maximized' : ''}`}>
          <RightPanel compact mode={rightMode} tabs={openRightTabs} detail={desktopConversation.detail} width={rightWidth} maximized={rightPanelMaximized} fileTabs={openFileTabs} activeFileTabId={activeFileTabId} activeChildAgent={activeChildAgent} onMode={selectRightMode} onCloseMode={closeRightMode} onOpenFile={openFile} onActivateFile={activateFile} onCloseFile={closeFile} onOpenChildAgent={openChildAgent} onBackChildAgent={() => setActiveChildAgent(null)} onOpenEvidence={openEvidence} onToggleMaximized={() => setRightPanelMaximized((value) => !value)} onCollapse={collapseRightSidebar} />
        </SheetContent>
      </Sheet>
      <KnowledgeBindingDialog open={knowledgeDialogOpen} remoteItems={knowledge.items} localItems={localKnowledgeBases} localLoading={localKnowledgeLoading} remoteLoading={knowledge.loading} remoteError={knowledge.error} onRetry={refreshKnowledgeChoices} references={desktopConversation.knowledgeReferences} bindings={desktopConversation.knowledgeBindings} busy={knowledgeDialogBusy} error={knowledgeDialogError} onOpenChange={setKnowledgeDialogOpen} onConfirm={(references, names) => void saveKnowledgeBindings(references, names)} />
      <ConversationManagementDialogs conversation={conversationDialog?.conversation ?? null} mode={conversationDialog?.mode ?? null} busy={conversationDialogBusy} error={conversationDialogError} onClose={() => { setConversationDialog(null); setConversationDialogError(null) }} onRename={(title) => void renameManagedConversation(title)} onDelete={() => void purgeManagedConversation()} />
      <ProjectDeleteDialog project={projectDeleteDialog} busy={projectDeleteBusy} onClose={() => setProjectDeleteDialog(null)} onDelete={() => void deleteManagedProject()} />
    </main>
  )
}
