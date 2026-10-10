import { type ActivityDetailPanelProps } from './components/ActivityDetailPanel'
import { FOX_ASSISTANT_AVATAR } from './components/FoxAssistantAvatar'
import { expertErrorMessage, expertInteractionLocked, latestExpertToolAvailability } from './components/expert-binding-ui'
import { useGoalProgress } from './hooks/use-goal-progress'
import { conversationModelOverride } from './model-selection'
import { canonicalProjectRoot, matchingProject, selectProjectRoot, validPickedProjectFolder } from './project-access-dialog-state'
import { EMPTY_RUNTIME_ARTIFACTS } from './runtime-timeline-performance'
import { type SidebarProjectGroup } from './sidebar-project-groups'
import { useVisibleCompletionAcknowledgement } from './use-conversation-completion-notices'
import { adoptBlockedStyles } from '@/components/effects/adopted-styles'
import { Sheet, SheetContent } from '@/components/ui/sheet'
import { asAgentCard } from '@/features/agents/agent-card-data'
import { chatSelectableAgents, normalizeAgentClassification, resolveActiveChatAgent } from '@/features/agents/agent-classification'
import { resolveExpertKnowledgeDeclaration } from '@/features/agents/expert-knowledge'
import { useAgents } from '@/features/agents/use-agents'
import { type LocalKnowledgeBaseDto, desktopClient, desktopErrorDetails, desktopRuntimeAvailable, knowledgeReferenceFromLegacyBinding, knowledgeReferenceKey } from '@/features/conversations/api/desktop-client'
import { latestMessage, runIsActive, useDesktopConversation } from '@/features/conversations/hooks/use-desktop-conversation'
import { useProjectCatalog } from '@/features/conversations/hooks/use-project-catalog'
import { conversationRunState } from '@/features/conversations/model/kernel-snapshot'
import { pendingRuntimeApprovals, pendingRuntimeQuestion } from '@/features/conversations/model/pending-interactions'
import { type ApprovalDecision, type ArtifactRecord, type ChildRunRecord, type ConversationMessage, type ConversationSummary, type KnowledgeBaseRecord, type KnowledgeReference, type ProjectRecord, type TaskEvidenceRecord } from '@/features/conversations/model/types'
import { useKnowledgeBases } from '@/features/knowledge/use-knowledge'
import { notify as toast } from '@/features/notifications'
import { useUserProfile } from '@/features/profile/user-profile'
import { useModelService } from '@/features/settings/use-model-service'
import { useYuxiModels } from '@/features/settings/use-yuxi-models'
import { useYuxiService } from '@/features/settings/use-yuxi-service'
import { useYuxiUser } from '@/features/settings/use-yuxi-user'
import { type ManagementReturnRoutes, captureManagementReturnRoutes, managementExitRoute } from '@/features/workspace/management-navigation'
import { type KnowledgeSourceLocator, type NavigateWorkspace, type WorkspaceView } from '@/features/workspace/types'
import { WorkspaceShell } from '@/features/workspace/workspace-shell'
import { File } from 'lucide-react'
import { type KeyboardEvent, Profiler, Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { MemoizedComposer, isChangeOrCommandTool, readStoredSeenChanges } from './composer/fox-composer'
import { ConversationManagementDialogs, KnowledgeBindingDialog, ProjectDeleteDialog } from './conversation-dialogs'
import { type AssistantMode, CONVERSATION_FORK_EVENT, type ChatState, type ChildAgentSelection, ComposerRuntimeContext, type ComposerRuntimeContextValue, EMPTY_CONVERSATION_ATTACHMENTS, EMPTY_CONVERSATION_MESSAGES, EMPTY_RUNTIME_APPROVALS, type OpenFileTab, type RightMode, TimelineRuntimeContext, type TimelineRuntimeContextValue, layoutStorage, readDefaultProjectPermission, readStoredBoolean, readStoredNumber, readStoredRightMode, readStoredRightTabs, recordRegionRender, useStableCallback } from './fox-shared'
import { RightPanel, artifactProjectPath, childAgentVisualQaPanelWidth, createOpenFileTab } from './right-panel/fox-right-panel'
import { ChatTopbar } from './shell/fox-chat-topbar'
import { MemoizedSidebar } from './shell/fox-sidebar'
import { MemoizedWindowTitlebar } from './shell/fox-window-titlebar'
import { type RuntimeSource } from './timeline/fox-runtime-process-model'
import { MemoizedTimeline } from './timeline/fox-timeline'
import { readChatEnginePreference, writeChatEnginePreference, type FoxChatEngine } from './thread/fox-chat-engine'
import './runtime-process-groups.css'
import './runtime-process-row.css'

const OnboardingPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.OnboardingPage })))

// Expert details open as a dialog inside chat (same dialog the experts page uses),
// so a look at an expert never navigates away from the conversation.
const AgentDetailDialog = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentDetailDialog })))

// The sidebar's running state uses the package's own orb. It stays a lazy chunk so
// the entry bundle keeps its budget; the 20px slot is reserved either way, so the
// row never shifts while the chunk lands.
const KernelReconciliationPanel = lazy(() => import('./kernel-reconciliation-panel').then((module) => ({ default: module.KernelReconciliationPanel })))

const ExpertPickerDialog = lazy(() => import('@/features/agents/ExpertPickerDialog').then((module) => ({ default: module.ExpertPickerDialog })))

// The assistant-ui chat branch ships as its own lazy chunk so the legacy path
// and the first-screen budget stay untouched while the migration runs.
const AssistantUiChat = lazy(() => import('./thread/fox-assistant-ui-chat'))
const ChatEngineSwitch = lazy(() => import('./thread/fox-assistant-ui-chat').then((module) => ({ default: module.ChatEngineSwitch })))

export const LOCAL_KNOWLEDGE_WORKSPACE_VIEWS: WorkspaceView[] = [
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
  const acknowledgeVisibleCompletion = useVisibleCompletionAcknowledgement({
    visible: activeView === 'chat',
    conversationId: desktopConversation.selectedConversationId,
    displayedConversationId: desktopConversation.detail?.conversation.id ?? null,
    acknowledgeCompletion: desktopConversation.acknowledgeCompletion,
  })
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
  const [focusedEvidence, setFocusedEvidence] = useState<TaskEvidenceRecord | null>(null)
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
  const { projects, setProjects, refreshProjects, setProjectPinned } = useProjectCatalog(desktopConversation.enabled, desktopConversation.detail?.conversation.projectId)
  const [chatState, setChatState] = useState<ChatState>('complete')
  const [activePrompt, setActivePrompt] = useState('Kun 的协议是什么协议，我可以拿来二次开发并且企业内部使用吗？')
  const [expertPickerOpen, setExpertPickerOpen] = useState(false)
  const [expertDetailId, setExpertDetailId] = useState<string | null>(null)
  const [composerResetKey, setComposerResetKey] = useState(0)
  const [composerDraft, setComposerDraft] = useState('')
  const [pendingUserMessage, setPendingUserMessage] = useState<ConversationMessage | null>(null)
  // Migration step 5B: `legacy` is the pre-5B implementation and the default;
  // `assistant-ui` mounts the external-store-driven thread instead.
  const [chatEngine, setChatEngine] = useState<FoxChatEngine>(readChatEnginePreference)
  const toggleChatEngine = (engine: FoxChatEngine) => {
    writeChatEnginePreference(engine)
    setChatEngine(engine)
  }
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
    ?? (desktopConversation.draftProjectRoot ? matchingProject(desktopConversation.draftProjectRoot, projects) : undefined)
    ?? null
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
  const activeRuntimeApprovals = useMemo(() => pendingRuntimeApprovals(desktopConversation.detail), [desktopConversation.detail])
  const pendingRuntimeApproval = activeRuntimeApprovals[0]
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
    setFocusedEvidence(null)
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
  // The host CSP carries a style nonce, which drops the stylesheets our effect
  // libraries inject at runtime; re-publish them as adopted (CSSOM) sheets.
  useEffect(() => adoptBlockedStyles(), [])
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
      setFocusedEvidence(evidence)
      selectRightMode(evidence.evidenceType === 'file_diff' ? 'changes' : 'activity')
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
  /**
   * The single entry point for starting a fresh draft.
   *
   * Everything that identifies "the session on screen" is reset together: the
   * conversation identity (already cleared by the hook), the run identity, the
   * composer text, the temporary optimistic message and the view selection. The
   * previous conversation is never cancelled — it keeps running in the background,
   * and its later events are routed by their own conversation id, so they cannot
   * touch this draft. Resetting piecemeal left a draft wearing the old run's
   * "stop" button over an empty composer.
   */
  const beginNewDraft = (options: {
    suggestion?: string
    mode?: AssistantMode
    /** Force the empty-composer layout even when a suggestion is pre-filled. */
    empty?: boolean
  } = {}) => {
    clearWorkflowTimer()
    setChatState('complete')
    setActivePrompt('')
    setPendingUserMessage(null)
    setEmptyConversation(options.empty ?? !options.suggestion)
    setComposerDraft(options.suggestion ?? '')
    setComposerResetKey((value) => value + 1)
    setActiveEntityId(null)
    setActiveDocumentId(null)
    setActiveSourceLocator(null)
    if (options.mode) setAssistantMode(options.mode)
    setActiveView('chat')
  }
  const resetConversation = (suggestion = '') => {
    if (desktopConversation.enabled) {
      void desktopConversation.createConversation()
    }
    beginNewDraft({ suggestion })
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
      beginNewDraft({ mode, empty: true })
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
    // The directory that was asked for wins over whatever conversation is open, and
    // it is matched on its full path so a same-named folder elsewhere cannot be used.
    const requestedProject = requestedProjectRoot ? matchingProject(requestedProjectRoot, projects) : activeProject
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
    desktopConversation.selectProject(canonicalProjectRoot(projectRoot), permissionMode)
    beginNewDraft()
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
    beginNewDraft()
    toast.success(`项目“${projectSelection.path.split(/[\\/]/).filter(Boolean).at(-1) ?? projectSelection.path}”已加入当前对话`)
  }
  const changeProjectPermission = async (permissionMode: ProjectRecord['permissionMode']) => {
    const conversationId = desktopConversation.detail?.conversation.id
    const projectId = desktopConversation.detail?.conversation.projectId ?? activeProject?.id
    try {
      if (conversationId) {
        // A conversation (including project-backed conversations) changes policy
        // through a versioned local-user request. Conflicts never auto-retry.
        const policy = await desktopClient.conversationPermissionState(conversationId)
        await desktopClient.updateConversationPermission(
          conversationId, permissionMode, crypto.randomUUID(), policy.version,
        )
        await desktopConversation.openConversation(conversationId)
        if (projectId) await refreshProjects()
      } else if (projectId) {
        const updated = await desktopClient.updateProjectPermission(projectId, permissionMode)
        setProjects((current) => current.some((project) => project.id === updated.id)
          ? current.map((project) => project.id === updated.id ? { ...updated, pinned: project.pinned } : project)
          : [...current, updated])
      }
      desktopConversation.setDraftPermission(permissionMode)
    } catch (cause) {
      // Keep the displayed authority unchanged on a rejected CAS. The user
      // decides whether to refresh and submit a new logical request.
      toast.error(cause instanceof Error ? cause.message : String(cause))
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
  const pinManagedProject = async (project: SidebarProjectGroup) => {
    if (!project.id) return
    try {
      await setProjectPinned(project.id, !project.pinned)
      toast.success(project.pinned ? '已取消项目置顶' : '项目已置顶')
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : '无法更新项目置顶状态')
    }
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
    ? <Suspense fallback={null}><KernelReconciliationPanel key={recoveryRunId} conversationId={recoveryConversationId} runId={recoveryRunId} onResumed={handleRecoveryResumed} /></Suspense>
    : null, [recoveryConversationId, recoveryRunId, handleRecoveryResumed])
  const timelineRuntimeContextValue = useMemo<TimelineRuntimeContextValue>(() => ({
    knowledgeBindings: desktopConversation.knowledgeBindings,
    assistantName: timelineAssistantName,
    runModel: timelineRunModel,
    onOpenSource: handleRuntimeOpenSource,
    recoveryPanel,
    userProfile,
    authoritativeRunId: desktopConversation.detail?.kernelSnapshot?.runId,
    authoritativeRunState: desktopConversation.detail?.kernelSnapshot?.state,
    deliveryRunId: desktopConversation.detail?.lastRun?.id,
    deliveryRunState: conversationRunState(desktopConversation.detail),
    deliveryChecklist: desktopConversation.detail?.deliveryChecklist,
    deliveryChecklistTruncated: desktopConversation.detail?.deliveryChecklistTruncated,
  }), [desktopConversation.knowledgeBindings, desktopConversation.detail?.kernelSnapshot?.runId, desktopConversation.detail?.kernelSnapshot?.state,
    desktopConversation.detail?.lastRun?.id, desktopConversation.detail?.lastRun?.status, desktopConversation.detail?.deliveryChecklist,
    desktopConversation.detail?.deliveryChecklistTruncated, handleRuntimeOpenSource, timelineAssistantName, timelineRunModel, recoveryPanel,
    userProfile.name, userProfile.avatar, userProfile.initial])
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
  // Same "latest revision wins" rule the legacy timeline used for its floater.
  const assistantUiPlanRevision = useMemo(() => {
    const plans = goalProgressData?.planRevisions
    if (!plans?.length) return undefined
    return plans.reduce((latest, plan) => !latest || plan.revision > latest.revision ? plan : latest)
  }, [goalProgressData?.planRevisions])
  // A tool row's file name opens in the right sidebar file view, exactly like an
  // artifact card, and its context menu reveals the same path in the OS explorer.
  const handleTimelineOpenToolFile = useStableCallback((path: string) => {
    openFile(path)
  })
  const handleTimelineRevealToolFile = useStableCallback((path: string) => {
    const conversationId = desktopConversation.detail?.conversation.id
    if (!conversationId) {
      toast.info('当前对话还没有可打开的本地项目')
      return
    }
    void desktopClient.projectFileAction(conversationId, path, 'reveal')
      .catch((cause) => toast.error(desktopErrorDetails(cause).message))
  })
  const handleTimelineViewExpert = useStableCallback((expertId: string) => setExpertDetailId(expertId))
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
    if (activeExpert) setExpertDetailId(activeExpert.id)
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
  const handleComposerResolveWorkflowGate = useStableCallback((workflowRunId: string, stageId: string, decision: 'approved' | 'rejected') => desktopConversation.resolveExpertWorkflowGate(workflowRunId, stageId, decision))
  const handleComposerDeleteGoal = useStableCallback((goalId: string) => desktopConversation.deleteGoal(goalId))
  const handleComposerGoalRunningChange = useStableCallback((goalId: string, expectedVersion: number, running: boolean) => desktopConversation.setGoalRunning(goalId, expectedVersion, running))
  const handleComposerEvidenceClick = useStableCallback((evidence: TaskEvidenceRecord) => openEvidence(evidence))
  const activityPanelProps: Omit<ActivityDetailPanelProps, 'detail'> = {
    goal: goalProgressData,
    focusedEvidence,
    onEvidenceClick: handleComposerEvidenceClick,
    onDeleteGoal: handleComposerDeleteGoal,
    onGoalRunningChange: handleComposerGoalRunningChange,
  }
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
  const handlePinProject = useStableCallback((project: SidebarProjectGroup) => {
    void pinManagedProject(project)
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

  if (activeView === 'onboarding') return <Suspense fallback={<div className="fox-app-loading"><img src={FOX_ASSISTANT_AVATAR} alt="" /><p>Fox 正在准备引导页</p></div>}><OnboardingPage dark={dark} navigate={navigate} onExit={finishOnboarding} /></Suspense>

  return (
    <main className="fox-shell" style={shellStyle} onPointerDownCapture={acknowledgeVisibleCompletion} onWheelCapture={acknowledgeVisibleCompletion} onKeyDownCapture={acknowledgeVisibleCompletion}>
      <MemoizedWindowTitlebar leftSidebarCollapsed={sidebarCollapsed || compactLayout} onNewChat={handleNewChat} onOpenProject={handleComposerProject} onSettings={handleOpenSettings} onAbout={handleOpenAbout} onToggleSidebar={handleToggleSidebar} onZoom={handleZoom} />
      <div className="fox-workbench">
        <MemoizedSidebar collapsed={sidebarCollapsed || compactLayout} assistantMode={assistantMode} onModeChange={handleAssistantModeChange} onNewChat={handleNewChat} onNewProjectChat={handleNewProjectChat} onAddProject={handleComposerProject} onCreateLocalKnowledge={handleCreateLocalKnowledge} onOpenConversation={handleOpenConversation} onRenameConversation={handleRenameConversation} projects={projects} onPinProject={handlePinProject} onPinConversation={handlePinConversation} onArchiveConversation={handleArchiveConversation} onUnarchiveConversation={handleUnarchiveConversation} onTrashConversation={handleTrashConversation} onRestoreConversation={handleRestoreConversation} onPurgeConversation={handlePurgeConversation} onDeleteProject={handleDeleteProject} onNavigate={handleWorkspaceNavigate} onExitManagement={handleExitManagement} onExpand={handleExpandSidebar} onTheme={handleToggleTheme} onSettings={handleOpenSettings} runtimeConversations={desktopConversation.enabled ? desktopConversation.conversations : undefined} archivedConversations={desktopConversation.enabled ? desktopConversation.archivedConversations : undefined} trashedConversations={desktopConversation.enabled ? desktopConversation.trashedConversations : undefined} onGlobalSearch={desktopRuntimeAvailable ? desktopClient.globalSearch : undefined} activeConversationId={desktopConversation.selectedConversationId ?? undefined} newChatActive={activeView === 'chat' && timelineEmpty && !desktopConversation.selectedConversationId} runIndicators={desktopConversation.enabled ? desktopConversation.runIndicators : undefined} yuxiService={sidebarYuxiService} yuxiUser={yuxiUser.user} activeView={activeView} activeEntityId={activeEntityId} activeDocumentId={activeDocumentId} />
        {!sidebarCollapsed && !compactLayout && <SidebarResizeDivider onResize={(delta) => setSidebarWidth((value) => Math.min(456, Math.max(220, value + delta)))} />}
        <div className="fox-content-card">
        <div className={`fox-content-surface ${rightPanelMaximized && showConversationRightSidebar && !rightSidebarCollapsed ? 'is-right-maximized' : ''}`}>
        <section className="fox-chat-pane">
          {expertPickerOpen && <Suspense fallback={null}><ExpertPickerDialog open={expertPickerOpen} onOpenChange={setExpertPickerOpen} agents={agentResource.agents} selectedExpertId={desktopConversation.selectedExpertId} readOnly={expertReadOnly} onSelect={selectExpert} /></Suspense>}
          {expertDetailId && <Suspense fallback={null}><AgentDetailDialog
            agent={(() => { const record = agentResource.agents.find((item) => item.id === expertDetailId); return record ? asAgentCard(record) : null })()}
            open
            onOpenChange={(next) => { if (!next) setExpertDetailId(null) }}
            onManage={() => { const id = expertDetailId; setExpertDetailId(null); navigate('agent-detail', id, { documentId: 'home' }) }}
            onUse={() => { const id = expertDetailId; setExpertDetailId(null); void selectExpert(id) }}
          /></Suspense>}
          <TimelineRuntimeContext.Provider value={timelineRuntimeContextValue}>
          <ComposerRuntimeContext.Provider value={composerRuntimeContextValue}>
          {workspacePage ?? <>{!timelineEmpty && <ChatTopbar rightSidebarCollapsed={rightSidebarCollapsed} onRightSidebarExpand={openRightSidebarHome} onOpenRightMode={selectRightMode} conversation={desktopConversation.detail?.conversation} detail={desktopConversation.detail} modelName={timelineRunModel || modelService.service?.modelId || (activeAgent?.defaultModel !== 'configured-model' ? activeAgent?.defaultModel : undefined) || '未配置模型'} state={chatState} onPinConversation={(conversation) => void pinManagedConversation(conversation)} onRenameConversation={(conversation) => setConversationDialog({ conversation, mode: 'rename' })} onArchiveConversation={(conversation) => void archiveManagedConversation(conversation)} />}
<div className={`fox-chat-stage ${timelineEmpty ? 'is-empty' : ''}`}>{chatEngine === 'assistant-ui' ? (<Suspense fallback={<div className="fox-app-loading"><img src={FOX_ASSISTANT_AVATAR} alt="" /><p>Fox 正在准备新界面</p></div>}><div className="fox-thread-shell"><ChatEngineSwitch engine={chatEngine} onChange={toggleChatEngine} /><AssistantUiChat key={desktopConversation.detail?.conversation.id ?? 'draft'} conversation={desktopConversation} pendingMessage={pendingUserMessage} isSendDisabled={expertReadOnly} suggestions={activeAgent?.openingSuggestions} planRevision={assistantUiPlanRevision} onOpenArtifact={handleTimelineOpenArtifact} onOpenFileInSidebar={handleTimelineOpenToolFile} onRevealFileInExplorer={handleTimelineRevealToolFile} callbacks={{ onSubmit: (prompt, files) => handleComposerSubmitPrompt(prompt, undefined, files), onOptimistic: handleComposerPromptCommit, onEdit: (messageId, prompt) => handleTimelineRerun(messageId, prompt), onReload: () => retryLatestRun() }} /></div></Suspense>) : (<><Profiler id="conversation-timeline" onRender={recordRegionRender}><MemoizedTimeline hasEarlierMessages={desktopConversation.detail?.hasEarlierMessages} loadingEarlierMessages={desktopConversation.loadingEarlierMessages} onLoadEarlierMessages={handleTimelineLoadEarlierMessages} empty={timelineEmpty} state={chatState} prompt={visiblePrompt} openingSuggestions={activeAgent?.openingSuggestions} runtimeMessages={desktopConversation.enabled ? desktopConversation.detail?.messages ?? EMPTY_CONVERSATION_MESSAGES : undefined} runtimeAttachments={desktopConversation.enabled ? desktopConversation.detail?.attachments ?? EMPTY_CONVERSATION_ATTACHMENTS : undefined} runtimeArtifacts={desktopConversation.enabled ? desktopConversation.detail?.artifacts ?? EMPTY_RUNTIME_ARTIFACTS : undefined} expertBindings={desktopConversation.expertBindings} agents={agentResource.agents} pendingMessage={pendingUserMessage} runtimeEvents={desktopConversation.enabled ? desktopConversation.detail?.runtimeEvents : undefined} runtimeRuns={desktopConversation.enabled ? desktopConversation.detail?.runs : undefined} runtimeRunId={desktopConversation.detail?.lastRun?.id} runtimeRunning={desktopRunning} runtimeReply={visibleReply} runtimeError={desktopConversation.error} runtimeErrorDetails={desktopConversation.errorDetails} planRevisions={goalProgressData?.planRevisions} onApprove={handleTimelineApprove} onDeny={handleTimelineDeny} onRetry={handleTimelineRetry} onRerun={handleTimelineRerun} onAnswer={handleTimelineAnswer} onStart={handleTimelineStart} onOpenArtifact={handleTimelineOpenArtifact} onViewExpert={handleTimelineViewExpert} onOpenFileInSidebar={handleTimelineOpenToolFile} onRevealFileInExplorer={handleTimelineRevealToolFile} /></Profiler><Profiler id="conversation-composer" onRender={recordRegionRender}><MemoizedComposer resetKey={composerResetKey} draft={composerDraft} setDraft={setComposerDraft} chatState={chatState} centered={timelineEmpty} dark={dark} runtimeControlled={desktopConversation.enabled} runtimeInitializing={desktopConversation.enabled && !desktopConversation.ready && !desktopConversation.error} projectRoot={desktopConversation.detail?.conversation.projectRoot ?? desktopConversation.draftProjectRoot} projectPermissionMode={desktopConversation.detail?.conversation.permissionMode ?? activeProject?.permissionMode ?? desktopConversation.draftPermissionMode} activeAgent={activeAgent} activeExpert={activeExpert} expertReadOnly={expertReadOnly} expertToolAvailability={expertToolAvailability} agents={agentResource.agents} modelService={modelService.service} runtimeCapabilities={desktopConversation.runtimeStatus?.capabilities} yuxiModels={yuxiModels.models} goalProgressData={goalProgressData} runtimeApprovals={desktopConversation.enabled ? activeRuntimeApprovals : EMPTY_RUNTIME_APPROVALS} steeringConversationId={desktopConversation.enabled ? desktopConversation.detail?.conversation.id : undefined} steeringActive={desktopConversation.enabled && desktopRunning} onProject={handleComposerProject} onPermissionModeChange={handleComposerPermissionModeChange} onAgentChange={handleComposerAgentChange} onViewExpert={handleComposerViewExpert} onChangeExpert={handleComposerChangeExpert} onRemoveExpert={handleComposerRemoveExpert} onHeightChange={setComposerHeight} onPromptCommit={handleComposerPromptCommit} onSubmitPrompt={handleComposerSubmitPrompt} onApprove={handleTimelineApprove} onDeny={handleTimelineDeny} onAnswer={handleTimelineAnswer} onResolveApproval={handleComposerResolveApproval} onResolveWorkModeConfirmation={handleComposerResolveWorkModeConfirmation} onResolvePlanRevision={handleComposerResolvePlanRevision} onResolveWorkflowGate={handleComposerResolveWorkflowGate} onEvidenceClick={handleComposerEvidenceClick} onStatusChange={handleComposerStatusChange} onCancel={desktopConversation.enabled ? handleComposerCancel : undefined} /></Profiler></>)}</div></>}
          </ComposerRuntimeContext.Provider>
          </TimelineRuntimeContext.Provider>
        </section>
        {showConversationRightSidebar && !rightSidebarCollapsed && !compactLayout && <>{!rightPanelMaximized && <ResizeDivider onResize={(delta) => setRightWidth((value) => Math.min(760, Math.max(280, value + delta)))} />}<RightPanel mode={rightMode} tabs={openRightTabs} detail={desktopConversation.detail} activity={activityPanelProps} width={rightWidth} maximized={rightPanelMaximized} fileTabs={openFileTabs} activeFileTabId={activeFileTabId} activeChildAgent={activeChildAgent} onMode={selectRightMode} onCloseMode={closeRightMode} onOpenFile={openFile} onActivateFile={activateFile} onCloseFile={closeFile} onOpenChildAgent={openChildAgent} onBackChildAgent={() => setActiveChildAgent(null)} onOpenEvidence={openEvidence} onToggleMaximized={() => setRightPanelMaximized((value) => !value)} onCollapse={collapseRightSidebar} /></>}
        </div>
        </div>
      </div>
      <Sheet open={showConversationRightSidebar && !rightSidebarCollapsed && compactLayout && compactRightOpen} onOpenChange={(open) => { if (open) setCompactRightOpen(true); else collapseRightSidebar() }}>
        <SheetContent side="right" showCloseButton={false} className={`fox-compact-context-sheet ${rightPanelMaximized ? 'is-maximized' : ''}`}>
          <RightPanel compact mode={rightMode} tabs={openRightTabs} detail={desktopConversation.detail} activity={activityPanelProps} width={rightWidth} maximized={rightPanelMaximized} fileTabs={openFileTabs} activeFileTabId={activeFileTabId} activeChildAgent={activeChildAgent} onMode={selectRightMode} onCloseMode={closeRightMode} onOpenFile={openFile} onActivateFile={activateFile} onCloseFile={closeFile} onOpenChildAgent={openChildAgent} onBackChildAgent={() => setActiveChildAgent(null)} onOpenEvidence={openEvidence} onToggleMaximized={() => setRightPanelMaximized((value) => !value)} onCollapse={collapseRightSidebar} />
        </SheetContent>
      </Sheet>
      <KnowledgeBindingDialog open={knowledgeDialogOpen} remoteItems={knowledge.items} localItems={localKnowledgeBases} localLoading={localKnowledgeLoading} remoteLoading={knowledge.loading} remoteError={knowledge.error} onRetry={refreshKnowledgeChoices} references={desktopConversation.knowledgeReferences} bindings={desktopConversation.knowledgeBindings} busy={knowledgeDialogBusy} error={knowledgeDialogError} onOpenChange={setKnowledgeDialogOpen} onConfirm={(references, names) => void saveKnowledgeBindings(references, names)} />
      <ConversationManagementDialogs conversation={conversationDialog?.conversation ?? null} mode={conversationDialog?.mode ?? null} busy={conversationDialogBusy} error={conversationDialogError} onClose={() => { setConversationDialog(null); setConversationDialogError(null) }} onRename={(title) => void renameManagedConversation(title)} onDelete={() => void purgeManagedConversation()} />
      <ProjectDeleteDialog project={projectDeleteDialog} busy={projectDeleteBusy} onClose={() => setProjectDeleteDialog(null)} onDelete={() => void deleteManagedProject()} />
    </main>
  )
}

export { Sidebar } from './shell/fox-sidebar'
export { RuntimeTimeline } from './timeline/fox-runtime-timeline'
export { RightPanel } from './right-panel/fox-right-panel'
export { KnowledgeBindingDialog } from './conversation-dialogs'
