import { FoxRunStatusBar } from './components/FoxRunStatusBar'
import { AgentBackendPanel } from './components/AgentBackendPanel'
import { AGENT_BACKEND_REFUSED, agentBackendInputLockedReason, conversationAgentBackend } from '@/features/conversations/model/agent-backend'
import { type ActivityDetailPanelProps } from './components/ActivityDetailPanel'
import { expertErrorMessage, expertInteractionLocked, latestExpertToolAvailability } from './components/expert-binding-ui'
import { useGoalProgress } from './hooks/use-goal-progress'
import { conversationModelOverride } from './model-selection'
import { localKnowledgeDocumentFile } from './local-knowledge-document-draft'
import type { ComposerInitialAttachments } from './composer/composer-initial-attachments'
import { canonicalProjectRoot, matchingProject, selectProjectRoot, validPickedProjectFolder } from './project-access-dialog-state'
import { EMPTY_RUNTIME_ARTIFACTS } from './runtime-timeline-performance'
import { type SidebarProjectGroup } from './sidebar-project-groups'
import { useVisibleCompletionAcknowledgement } from './use-conversation-completion-notices'
import { RuntimeArtifacts } from './runtime-artifacts'
import { DeliveryAcceptanceCard } from './components/DeliveryAcceptanceCard'
import { adoptBlockedStyles } from '@/components/effects/adopted-styles'
import { Button } from '@/components/ui/button'
import { asAgentCard } from '@/features/agents/agent-card-data'
import { chatSelectableAgents, normalizeAgentClassification, resolveActiveChatAgent } from '@/features/agents/agent-classification'
import { resolveExpertKnowledgeDeclaration } from '@/features/agents/expert-knowledge'
import { useAgents } from '@/features/agents/use-agents'
import { type LocalKnowledgeBaseDto, DesktopCommandError, desktopClient, desktopErrorDetails, desktopRuntimeAvailable, knowledgeReferenceFromLegacyBinding, knowledgeReferenceKey } from '@/features/conversations/api/desktop-client'
import { type ComposerAccessMode, conversationAccessMode, hostAccessMode, storedPermissionMode } from './access-mode'
import { latestMessage, runIsActive, useDesktopConversation } from '@/features/conversations/hooks/use-desktop-conversation'
import { useProjectCatalog } from '@/features/conversations/hooks/use-project-catalog'
import { conversationRunState } from '@/features/conversations/model/kernel-snapshot'
import { pendingRuntimeApprovals, pendingRuntimeQuestion } from '@/features/conversations/model/pending-interactions'
import { lastAgentBackendFailure } from '@/features/conversations/model/agent-backend-failure'
import { type ApprovalDecision, type ArtifactRecord, type ChildRunRecord, type ConversationMessage, type ConversationSummary, type KnowledgeBaseRecord, type KnowledgeReference, type ProjectRecord, type TaskEvidenceRecord } from '@/features/conversations/model/types'
import { useKnowledgeBases } from '@/features/knowledge/use-knowledge'
import { notify as toast } from '@/features/notifications'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { useUserProfile } from '@/features/profile/user-profile'
import { useModelService } from '@/features/settings/use-model-service'
import { matchesShortcutAction } from '@/features/settings/shortcut-model'
import { useYuxiModels } from '@/features/settings/use-yuxi-models'
import { useYuxiService } from '@/features/settings/use-yuxi-service'
import { useYuxiUser } from '@/features/settings/use-yuxi-user'
import { type ManagementReturnRoutes, captureManagementReturnRoutes, managementExitRoute, settingsRouteRedirect } from '@/features/workspace/management-navigation'
import { type KnowledgeSourceLocator, type LocalKnowledgeDocumentAsk, type NavigateWorkspace, type WorkspaceView } from '@/features/workspace/types'
import { WorkspaceShell } from '@/features/workspace/workspace-shell'
import { useNavigationHistory } from '@/features/workspace/use-navigation-history'
import { File, ShieldAlert, ShieldCheck } from 'lucide-react'
import { type KeyboardEvent, type ReactNode, Profiler, Suspense, lazy, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { MemoizedComposer, isChangeOrCommandTool, readStoredSeenChanges } from './composer/fox-composer'
import { ComposerEntryContext } from './composer/composer-entry-context'
import { ConversationManagementDialogs, KnowledgeBindingDialog, ProjectDeleteDialog } from './conversation-dialogs'
import { type AssistantMode, CONVERSATION_FORK_EVENT, type ChatState, type ChildAgentSelection, ComposerRuntimeContext, type ComposerRuntimeContextValue, EMPTY_CONVERSATION_ATTACHMENTS, EMPTY_CONVERSATION_MESSAGES, EMPTY_RUNTIME_APPROVALS, type OpenFileTab, type RightMode, TimelineRuntimeContext, type TimelineRuntimeContextValue, layoutStorage, readDefaultProjectPermission, readStoredBoolean, readStoredNumber, readStoredRightMode, readStoredRightTabs, recordRegionRender, useStableCallback } from './fox-shared'
import { RightPanel, artifactProjectPath, childAgentVisualQaPanelWidth, createOpenFileTab } from './right-panel/fox-right-panel'
import { ChatTopbar } from './shell/fox-chat-topbar'
import { MemoizedSidebar } from './shell/fox-sidebar'
import { MemoizedWindowTitlebar } from './shell/fox-window-titlebar'
import { type RuntimeSource } from './timeline/fox-runtime-process-model'
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
const ExpertLanyard = lazy(() => import('./components/ExpertLanyard').then((module) => ({ default: module.ExpertLanyard })))

// The assistant-ui chat branch ships as its own lazy chunk so the legacy path
// and the first-screen budget stay untouched while the migration runs.
const AssistantUiChat = lazy(() => import('./thread/fox-assistant-ui-chat'))

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
  'local-knowledge-settings',
]

/**
 * The composer line a library document arrives with: a readable one-liner whose
 * trailing newline leaves the caret on a free line, ready for the actual question.
 */
export function localKnowledgeDocumentPrompt(payload: LocalKnowledgeDocumentAsk): string {
  const location = payload.relativePath?.trim()
  return `【资料：${payload.documentName}${location ? ` · ${location}` : ''}】\n`
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

function SidebarResizeDivider({ width, onResize }: { width: number; onResize: (delta: number) => void }) {
  const start = useRef<number | null>(null)
  return <div className="fox-sidebar-resize-divider" role="separator" tabIndex={0} aria-label="调整左侧导航栏宽度" aria-orientation="vertical" aria-valuemin={220} aria-valuemax={456} aria-valuenow={Math.round(width)}
    onPointerDown={(event) => { if (event.button !== 0) return; event.preventDefault(); start.current = event.clientX; event.currentTarget.setPointerCapture(event.pointerId) }}
    onPointerMove={(event) => { if (start.current === null) return; const delta = event.clientX - start.current; start.current = event.clientX; onResize(delta) }}
    onPointerUp={() => { start.current = null }} onPointerCancel={() => { start.current = null }} onLostPointerCapture={() => { start.current = null }}
    onKeyDown={(event) => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); onResize((event.key === 'ArrowLeft' ? -1 : 1) * (event.shiftKey ? 24 : 8)) } else if (event.key === 'Home' || event.key === 'End') { event.preventDefault(); onResize((event.key === 'Home' ? 220 : 456) - width) } }} />
}

// Keep the compact drawer inside the content card: its backdrop and maximized
// width share the chat's bounds, while history navigation stays reachable.
function CompactRightPanel({ children, maximized, onClose }: { children: ReactNode; maximized: boolean; onClose: () => void }) {
  const panel = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null
    panel.current?.focus({ preventScroll: true })
    return () => { if (previous?.isConnected) previous.focus({ preventScroll: true }) }
  }, [])
  return <div className="fox-compact-context-layer">
    <button type="button" tabIndex={-1} className="fox-compact-context-backdrop" aria-label="收起右侧边栏" onClick={onClose} />
    <div ref={panel} role="region" aria-label="对话右侧面板" tabIndex={-1} className={`fox-compact-context-sheet ${maximized ? 'is-maximized' : ''}`} onKeyDown={(event) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose() }
    }}>{children}</div>
  </div>
}

/**
 * Package 2-D U1: the surface that asks the user to decide about deliverables
 * whose content the Host could not verify.
 *
 * Rendered while the Run is parked on `awaiting_delivery_review`, **and** while a
 * Run that has already ended still has an acceptance the user may want to
 * withdraw. The second half is R-1: gating purely on `parked` made the whole
 * panel — including the card's "撤销接受" button — disappear the moment the Run
 * completed, so the user could find neither their acceptance nor a way to undo
 * it.
 *
 * It reads the checklist that `load_conversation` already returns (no extra read)
 * and asks the Host for each outstanding item's acceptance state on demand — the
 * freshness check behind that state re-reads the candidate file, so it must not
 * run inside the bulk conversation load.
 *
 * It never renders an item as passed. The verification class and the acceptance
 * are two separate lines, because they are two separate facts.
 */
function DeliveryReviewPanel() {
  const { deliveryRunId, deliveryRunState, deliveryChecklist } = useContext(TimelineRuntimeContext)
  const parked = deliveryRunState === 'awaiting_delivery_review'
  // `recognition:*` rows are not deliverables: they record that the task text did
  // not name one. The Host refuses their acceptance and their binding, so rendering
  // a card for them would only produce a button that can never work. They stay in
  // the Run's outstanding list — that is the durable record that the Run is not
  // done — and are shown as the open question instead of as an acceptable item.
  const recognition = useMemo(
    () => (deliveryChecklist ?? []).filter((item) => item.runId === deliveryRunId && item.itemKey.startsWith('recognition:')),
    [deliveryChecklist, deliveryRunId],
  )
  const outstanding = useMemo(
    () => (deliveryChecklist ?? []).filter((item) => item.runId === deliveryRunId && item.status !== 'passed' && !item.itemKey.startsWith('recognition:')),
    [deliveryChecklist, deliveryRunId],
  )
  // A finished Run keeps the panel only while one of its rows still carries an
  // acceptance to withdraw. Probing the Host for that would re-read every
  // candidate file on each conversation load, which is exactly what the on-demand
  // `loadState` exists to avoid — so the panel renders whenever the Run is parked
  // or has ended with outstanding rows, and the **card** decides per row which
  // buttons exist. A Run with no outstanding rows at all renders nothing.
  if (!deliveryRunId || (outstanding.length === 0 && recognition.length === 0)) return null
  if (!parked && deliveryRunState !== 'completed' && deliveryRunState !== 'failed') return null
  const ended = !parked
  return (
    <div className="fox-delivery-review">
      <header>
        {ended ? <ShieldCheck size={15} /> : <ShieldAlert size={15} />}
        <div>
          <strong>
            {recognition.length > 0
              ? '交付要求未识别——需要你补充'
              : ended ? '交付内容尚未核验——任务已结束' : '交付内容尚未核验'}
          </strong>
          <small>
            {recognition.length > 0
              ? '任务提到了要做文件，但没说清具体交付什么，Host 没有可核验的对象。补充文件名、类型或目录后重发，会按新任务重新识别并重新核验；本次不会自动重跑模型，也不会被记成未通过。'
              : ended
                ? '该任务的核验结论保留原样。你此前的接受仍在，可以逐项撤销；任务不会因此重启或重跑。'
                : '产物已保留。你可以逐项接受未核验风险，或补充材料后重新核验；不会自动重跑模型修复。'}
          </small>
        </div>
      </header>
      {recognition.map((item) => {
        const reason = typeof item.finding?.reason === 'string' ? item.finding.reason : null
        return (
          <p className="fox-delivery-review__question" key={`${item.runId}:${item.itemKey}`}>
            {reason || '请明确要交付的文件名、类型或目录；当前没有可靠目标可自动修复'}
          </p>
        )
      })}
      {outstanding.map((item) => (
        <DeliveryAcceptanceCard
          key={`${item.runId}:${item.itemKey}`}
          runId={item.runId}
          itemKey={item.itemKey}
          description={item.description || item.targetPath || item.itemKey}
          loadState={(runId, itemKey) => desktopClient.deliveryAcceptanceState(runId, itemKey)}
          resolve={(runId, itemKey, decision) => desktopClient.resolveDeliveryAcceptance(runId, itemKey, decision)}        />
      ))}
    </div>
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
  const acknowledgeVisibleCompletion = useVisibleCompletionAcknowledgement({
    visible: activeView === 'chat',
    conversationId: desktopConversation.selectedConversationId,
    displayedConversationId: desktopConversation.detail?.conversation.id ?? null,
    acknowledgeCompletion: desktopConversation.acknowledgeCompletion,
  })
  const [localKnowledgeCreateRequest, setLocalKnowledgeCreateRequest] = useState(0)
  // Urgent half of the mode switch. Which tab the segmented control highlights
  // must not wait for the heavy half — switching modes tears the whole chat
  // thread off screen (or mounts it again), and that commit measured 120ms of
  // blocking work on this machine. Because the pill is driven from the same
  // state as `activeView`, it used to sit still for that entire time and then
  // move *together* with the page, which reads as "the switch is frozen".
  // Splitting the state lets the pill commit on its own, first.
  const [assistantMode, setAssistantMode] = useState<AssistantMode>('assistant')
  const [activeEntityId, setActiveEntityId] = useState<string | null>(null)
  const [activeDocumentId, setActiveDocumentId] = useState<string | null>(null)
  const [activeSourceLocator, setActiveSourceLocator] = useState<KnowledgeSourceLocator | null>(null)
  const documentAskRequest = useRef<{ version: number; abort: AbortController | null }>({ version: 0, abort: null })
  const [composerInitialAttachments, setComposerInitialAttachments] = useState<ComposerInitialAttachments | null>(null)
  const cancelDocumentAsk = () => {
    documentAskRequest.current.version += 1
    documentAskRequest.current.abort?.abort()
    documentAskRequest.current.abort = null
  }
  useEffect(() => () => documentAskRequest.current.abort?.abort(), [])
  useEffect(() => {
    if (documentAskRequest.current.abort) cancelDocumentAsk()
  }, [desktopConversation.selectedConversationId])
  const managementReturnRoutes = useRef<ManagementReturnRoutes>({
    settings: { view: 'chat', entityId: null, documentId: null },
    knowledge: { view: 'chat', entityId: null, documentId: null },
  })
  const onboardingReturnRoute = useRef<{ view: WorkspaceView; entityId: string | null; documentId: string | null }>({ view: 'chat', entityId: null, documentId: null })
  useEffect(() => {
    void desktopConversation.refreshRuntimeStatus()
  }, [desktopConversation.refreshRuntimeStatus, modelService.service?.supportsImageInput])
  const [sidebarManuallyCollapsed, setSidebarCollapsed] = useState(() => readStoredBoolean(layoutStorage.sidebarCollapsed, false))
  const [narrowSidebar, setNarrowSidebar] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 700px)').matches)
  const sidebarCollapsed = sidebarManuallyCollapsed || narrowSidebar
  // With the rail gone the sidebar's own content sets the width; the shell only
  // keeps the last expanded width so one number drives both sides of the motion.
  const [expandedSidebarWidth, setExpandedSidebarWidth] = useState(() => {
    // Apply the new default once, then retain subsequent manual adjustments.
    if (window.localStorage.getItem('fox.layout.sidebarWidthVersion') !== '260-v1') {
      window.localStorage.setItem(layoutStorage.sidebarWidth, '260')
      window.localStorage.setItem('fox.layout.sidebarWidthVersion', '260-v1')
    }
    return readStoredNumber(layoutStorage.sidebarWidth, 260, 220, 456)
  })
  useEffect(() => {
    const query = window.matchMedia('(max-width: 700px)')
    const sync = () => setNarrowSidebar(query.matches)
    sync()
    query.addEventListener('change', sync)
    return () => query.removeEventListener('change', sync)
  }, [])
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
  useEffect(() => {
    const handleFork = (event: Event) => {
      const messageId = (event as CustomEvent<{ messageId?: string }>).detail?.messageId
      if (!messageId) return
      void desktopConversation.forkConversation(messageId).then((fork) => {
        if (!fork) {
          toast.error(desktopConversation.error ?? '无法创建对话分支')
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
  // A Run parked on `awaiting_delivery_review` is still active for the Host (it
  // is continuable and not terminal), but it is not *working*: nothing is
  // dispatched and the review lane is asking the user to answer. Treating it as
  // busy turned the composer's submit control into Stop, so the clarification
  // the panel asks for could never be sent. `desktopBusy` is the narrower fact
  // the composer and the steering lane need: work is actually in flight.
  const desktopParkedForReview = desktopRunStatus === 'awaiting_delivery_review'
  const desktopBusy = desktopRunning && !desktopParkedForReview
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
  useEffect(() => { window.localStorage.setItem(layoutStorage.sidebarCollapsed, sidebarManuallyCollapsed ? '1' : '0') }, [sidebarManuallyCollapsed])
  useEffect(() => { window.localStorage.setItem(layoutStorage.sidebarWidth, String(Math.round(expandedSidebarWidth))) }, [expandedSidebarWidth])
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
      setCompactRightOpen(query.matches && !rightSidebarCollapsed)
    }
    syncLayout()
    query.addEventListener('change', syncLayout)
    return () => query.removeEventListener('change', syncLayout)
  }, [rightSidebarCollapsed])
  useEffect(() => () => {
    if (workflowTimer.current !== null) window.clearTimeout(workflowTimer.current)
  }, [])
  useEffect(() => {
    if (!desktopConversation.enabled) return
    if (pendingRuntimeApproval) setChatState('approval')
    else if (desktopConversation.error) setChatState('error')
    // A parked Run is not `running`: the composer must stay able to send the
    // clarification the review lane asks for.
    else if (desktopBusy) setChatState('running')
    else if (desktopAssistantMessage || desktopUserMessage) setChatState('complete')
  }, [desktopAssistantMessage, desktopBusy, desktopConversation.enabled, desktopConversation.error, desktopUserMessage, pendingRuntimeApproval])
  useEffect(() => {
    if (!pendingUserMessage || !desktopUserMessage) return
    if (desktopUserMessage.content !== pendingUserMessage.content) return
    if (desktopUserMessage.createdAt < pendingUserMessage.createdAt - 3000) return
    setPendingUserMessage(null)
  }, [desktopUserMessage, pendingUserMessage])

  const shellStyle = useMemo(() => {
    // While collapsed the rail is suppressed (the narrow-rail CSS cannot reach a
    // 0-width sidebar), so the centred column needs no offset. Both numbers move
    // on the same clock as the sidebar's width transition.
    const railWidth = sidebarCollapsed ? 0 : `min(${expandedSidebarWidth}px, 34vw)`
    return {
      '--fox-sidebar-width': `${expandedSidebarWidth}px`,
      '--fox-sidebar-center-offset': `calc(${railWidth} / 2)`,
      '--fox-composer-occlusion': `${composerHeight}px`,
    } as React.CSSProperties
  }, [composerHeight, expandedSidebarWidth, sidebarCollapsed])

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
    cancelDocumentAsk()
    setComposerInitialAttachments(null)
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
    cancelDocumentAsk()
    void desktopConversation.createConversationForAgent(agentId, projectRoot, permissionMode).then(async (created) => {
      if (!created) {
        toast.error(desktopConversation.error ?? '无法创建对话草稿')
        return
      }
      if (knowledgeBases.length && !await desktopConversation.setKnowledgeBindings(knowledgeBases)) {
        toast.error(desktopConversation.error ?? '无法为对话启用资料库')
        return
      }
      beginNewDraft({ mode, empty: true })
    })
  }
  const askKnowledgeBase = (knowledgeBase: KnowledgeBaseRecord) => {
    const knowledgeAgent = agentResource.agents.find((agent) => agent.runtimeType === 'yuxi' && agent.isDefault && agent.available)
      ?? agentResource.agents.find((agent) => agent.runtimeType === 'yuxi' && agent.available)
    if (!knowledgeAgent) {
      toast.error(yuxi.service?.credentialConfigured ? '资料库服务暂无可用的默认知识助手' : '请先登录资料库，再使用知识助手')
      return
    }
    prepareDraft(knowledgeAgent.id, 'knowledge', undefined, undefined, [knowledgeBase])
  }
  // One fallback chain, kept identical to the one switchAssistantMode uses for the
  // "New conversation" button so both entries land on the same local agent.
  const localAssistantAgent = () => agentResource.agents.find((agent) => agent.id === 'fox-general')
    ?? agentResource.agents.find((agent) => agent.runtimeType !== 'yuxi' && agent.isDefault)
    ?? agentResource.agents.find((agent) => agent.runtimeType !== 'yuxi')
    ?? null
  const askLocalKnowledgeDocument = async (payload: LocalKnowledgeDocumentAsk) => {
    const localAgent = localAssistantAgent()
    if (!desktopConversation.enabled || !localAgent) {
      toast.error('Fox 本机默认助手暂不可用')
      return
    }
    cancelDocumentAsk()
    const requestId = documentAskRequest.current.version
    const abort = new AbortController()
    documentAskRequest.current.abort = abort
    const isCurrent = () => !abort.signal.aborted && documentAskRequest.current.version === requestId
    try {
      const file = await localKnowledgeDocumentFile(payload, desktopClient, abort.signal)
      if (!isCurrent()) return
      const reference: KnowledgeReference = { source: 'local', providerKey: 'local', id: payload.knowledgeBaseId }
      const created = desktopConversation.createConversationDraftWithKnowledge(
        localAgent.id,
        [reference],
        { [knowledgeReferenceKey(reference)]: payload.knowledgeBaseName },
      )
      if (!created) {
        toast.error(desktopConversation.error ?? '无法创建对话草稿')
        return
      }
      beginNewDraft({ mode: 'assistant', suggestion: localKnowledgeDocumentPrompt({ ...payload, documentName: file.name }) })
      setComposerInitialAttachments({ requestId, files: [file] })
    } catch (cause) {
      if (isCurrent()) toast.error(desktopErrorDetails(cause).message)
    }
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
        toast.error('Fox 本机默认助手暂不可用')
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
  const changeProjectPermission = async (accessMode: ComposerAccessMode) => {
    const conversationId = desktopConversation.detail?.conversation.id
    const projectId = desktopConversation.detail?.conversation.projectId ?? activeProject?.id
    const permissionMode = storedPermissionMode(accessMode)
    try {
      if (conversationId) {
        // A conversation (including project-backed conversations) changes policy
        // through a versioned local-user request. Conflicts never auto-retry.
        // A1: four modes; a mid-Run switch applies from the next tool call.
        const policy = await desktopClient.conversationPermissionState(conversationId)
        try {
          await desktopClient.updateConversationPermission(
            conversationId, hostAccessMode(accessMode), crypto.randomUUID(), policy.version,
          )
        } catch (cause) {
          // The first full access in a project needs an explicit second
          // confirmation; the project remembers it afterwards.
          if (!(cause instanceof DesktopCommandError) || cause.code !== 'permission.full_access_confirmation_required') throw cause
          const confirmed = await confirmDialog({
            title: '为这个项目开启完全访问？',
            description: cause.message,
            confirmLabel: '开启完全访问',
            destructive: true,
          })
          if (!confirmed) return
          const fresh = await desktopClient.conversationPermissionState(conversationId)
          await desktopClient.updateConversationPermission(
            conversationId, hostAccessMode(accessMode), crypto.randomUUID(), fresh.version, true,
          )
        }
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
      if (!saved) throw new Error(desktopConversation.error ?? '无法保存资料库选择')
    } catch (cause) {
      setKnowledgeDialogBusy(false)
      setKnowledgeDialogError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
    setKnowledgeDialogBusy(false)
    if (closeDialog) setKnowledgeDialogOpen(false)
    toast.success(references.length ? `已为当前对话启用 ${references.length} 个资料库` : '已清除当前对话的资料库')
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
  // A context reset keeps the chat: only the model context starts over in a
  // new Pi session generation. The confirmation names what is kept.
  const resetManagedConversation = async (conversation: ConversationSummary) => {
    const confirmed = await confirmDialog({
      title: '重置会话上下文',
      description: '将开始一个新的 Pi 会话。历史消息、专家绑定、记忆与知识库绑定都会保留；旧会话文件仍保留在磁盘上，直到永久删除对话。',
      confirmLabel: '重置上下文',
    })
    if (!confirmed) return
    const result = await desktopConversation.resetConversationContext(conversation.id)
    if (!result.ok) {
      toast.error(result.error ?? '无法重置会话上下文')
      return
    }
    toast.success('会话上下文已重置，下一条消息将使用新的会话')
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
  const selectExpert = useStableCallback(async (expertId: string, newConversation = false): Promise<boolean> => {
    if (expertReadOnly) return false
    if (expertId === desktopConversation.selectedExpertId) return true
    if (!newConversation && expertId === activeAgent?.id) {
      if (!desktopConversation.selectedExpertId) return true
      const result = await desktopConversation.removeExpert()
      if (!result.success) toast.error(expertErrorMessage(result.error, '无法切回默认专家'))
      return result.success
    }
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
      toast.error('专家资料库配置无效', { description: cause instanceof Error ? cause.message : String(cause) })
      return false
    }
    const result = newConversation
      ? await desktopConversation.createConversationForExpert(targetAgent.id)
      : await desktopConversation.changeExpert(targetAgent.id)
    if (!result.success) {
      toast.error(expertErrorMessage(result.error, '无法与这位专家开始对话'))
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
        toast.error('专家已启用，但资料库绑定失败')
      }
    }
    return true
  })
  const navigate: NavigateWorkspace = (requestedView, entityId, requestedContext) => {
    cancelDocumentAsk()
    // Old settings ids (快捷键, 项目权限, 使用情况, 数据维护, 关于) now live inside
    // merged pages; land there and scroll to the matching section.
    const settingsRedirect = settingsRouteRedirect(requestedView)
    const view = settingsRedirect?.view ?? requestedView
    const context = settingsRedirect ? { ...requestedContext, documentId: requestedContext?.documentId ?? settingsRedirect.section } : requestedContext
    if (view === 'onboarding' && activeView !== 'onboarding') {
      onboardingReturnRoute.current = { view: activeView, entityId: activeEntityId, documentId: activeDocumentId }
    }
    if (view === 'chat' && entityId) {
      const targetAgent = agentResource.agents.find((item) => item.id === entityId)
      if (!targetAgent) return
      const targetKind = normalizeAgentClassification(targetAgent).agentKind
      if (targetKind === 'worker') return
      if (targetKind === 'expert') void selectExpert(entityId, true)
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
    cancelDocumentAsk()
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
      if (matchesShortcutAction(event, 'new-conversation')) {
        event.preventDefault()
        switchAssistantMode('assistant', true)
      } else if (matchesShortcutAction(event, 'add-project')) {
        event.preventDefault()
        void addProject()
      } else if (matchesShortcutAction(event, 'open-settings')) {
        event.preventDefault()
        navigate('settings')
      } else if (matchesShortcutAction(event, 'toggle-sidebar')) {
        event.preventDefault()
        setSidebarCollapsed((value) => !value)
      }
    }
    window.addEventListener('keydown', handleShortcut)
    return () => window.removeEventListener('keydown', handleShortcut)
  })

  const workspacePage = activeView === 'chat' || activeView === 'onboarding' ? null : (
    <WorkspaceShell
      activeView={activeView}
      sidebarCollapsed={sidebarCollapsed}
      onSidebar={() => setSidebarCollapsed(!sidebarCollapsed)}
      navigate={navigate}
      activeEntityId={activeEntityId}
      activeDocumentId={activeDocumentId}
      activeSourceLocator={activeSourceLocator}
      dark={dark}
      onDark={() => setDark(!dark)}
      onAskKnowledge={askKnowledgeBase}
      onAskLocalKnowledgeDocument={askLocalKnowledgeDocument}
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
    navigate(source.source === 'local' ? 'local-knowledge-documents' : 'knowledge-detail', source.knowledgeBaseId, { documentId: source.documentId, sourceLocator: source.locator })
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
      toast.info('当前对话还没有可打开的本机项目')
      return
    }
    void desktopClient.projectFileAction(conversationId, path, 'reveal')
      .catch((cause) => toast.error(desktopErrorDetails(cause).message))
  })
  const handleTimelineViewExpert = useStableCallback((expertId: string) => setExpertDetailId(expertId))
  const handleComposerProject = useStableCallback(() => {
    void addProject()
  })
  const handleEntryProjectSelect = useStableCallback((project: ProjectRecord) => {
    if (desktopConversation.selectedConversationId) return
    desktopConversation.selectProject(canonicalProjectRoot(project.rootPath), project.permissionMode)
  })
  const handleEntryProjectAdd = useStableCallback(async () => {
    if (desktopConversation.selectedConversationId) return
    const selected = await pickProjectFolder()
    if (!selected) return
    const selection = selectProjectRoot(selected, projects, readDefaultProjectPermission())
    desktopConversation.selectProject(selection.path, selection.permissionMode)
  })
  const handleViewDefaultExpert = useStableCallback(() => {
    if (activeAgent) setExpertDetailId(activeAgent.id)
  })
  const handleOpenComposerModelSettings = useStableCallback(() => navigate('settings-models'))
  const handleInitialAttachmentsConsumed = useStableCallback((requestId: number) => {
    setComposerInitialAttachments((request) => request?.requestId === requestId ? null : request)
  })
  const handleComposerPermissionModeChange = useStableCallback((accessMode: ComposerAccessMode) => changeProjectPermission(accessMode))
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
      if (started === AGENT_BACKEND_REFUSED) {
        // Backend routing refused the send before anything was written: no
        // optimistic turn, no "处理失败" header. The one refusal notice is
        // shown by the backend bar inside the composer; the draft is kept.
        setPendingUserMessage(null)
        setChatState('complete')
        return 'error' as const
      }
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
  // `startTransition` cannot help here. The mode state is lifted up in
  // `Workbench`, so updating it re-renders the whole workbench, and a transition
  // scheduled inside a discrete event is still flushed before the next paint —
  // the pill would wait for the heavy commit exactly as before.
  const handleAssistantModeChange = useStableCallback((mode: AssistantMode) => switchAssistantMode(mode))
  const handleNewChat = useStableCallback(() => switchAssistantMode('assistant', true))
  const handleNewProjectChat = useStableCallback((projectRoot?: string) => createProjectDraft(projectRoot))
  const handleCreateLocalKnowledge = useStableCallback(() => requestLocalKnowledgeCreate())
  const handleOpenConversation = useStableCallback((conversationId?: string) => {
    cancelDocumentAsk()
    setComposerInitialAttachments(null)
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
  const handleToggleSidebar = useStableCallback(() => { if (!narrowSidebar) setSidebarCollapsed((value) => !value) })
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
  // Hybrid rollback: both chat engines share the SAME classic composer. The
  // legacy composer talks to `useDesktopConversation()` directly, and the
  // assistant-ui thread subscribes to the same store, so sends from this one
  // instance stream into whichever message list is mounted.
  const composerEntry = useMemo(() => ({ projects, defaultExpert: activeAgent, onSelectProject: handleEntryProjectSelect, onAddProject: handleEntryProjectAdd, onViewDefaultExpert: handleViewDefaultExpert, onOpenModelSettings: handleOpenComposerModelSettings, draftAgentBackend: desktopConversation.draftAgentBackend, agentBackendStatus: desktopConversation.agentBackendStatus, onSelectAgentBackend: desktopConversation.setDraftAgentBackend }), [projects, activeAgent, handleEntryProjectSelect, handleEntryProjectAdd, handleViewDefaultExpert, handleOpenComposerModelSettings, desktopConversation.draftAgentBackend, desktopConversation.agentBackendStatus, desktopConversation.setDraftAgentBackend])
  // Plan v0.3.1 5.4 / 6.6: pick the backend for a new conversation, or show the
  // open conversation's frozen backend as locked. Only in the desktop runtime.
  // Fix 1 (Windows acceptance of 3a82f5d): the bar is rendered INSIDE the
  // composer container, directly above the input box, so it moves with the
  // composer and is counted in the composer height the timeline reserves.
  const openConversationSummary = desktopConversation.detail?.conversation ?? null
  const lockedAgentBackend = openConversationSummary ? conversationAgentBackend(openConversationSummary) : null
  const agentBackendStatus = desktopConversation.agentBackendStatus
  const agentBackendRefusalRecord = desktopConversation.agentBackendRefusal
  const visibleAgentBackendRefusal = agentBackendRefusalRecord
    && agentBackendRefusalRecord.conversationId === (desktopConversation.selectedConversationId ?? null)
    ? agentBackendRefusalRecord.message
    : null
  const composerInputLockedReason = desktopConversation.enabled && lockedAgentBackend
    ? agentBackendInputLockedReason(lockedAgentBackend, agentBackendStatus)
    : null
  // The draft picker can override the saved default for this conversation; the panel owns the
  // capability chips, permission, login probe, card and receipt, each keyed
  // by its target so a change of backend / project never shows stale data.
  const draftAgentBackend = desktopConversation.draftAgentBackend
  const openConversationId = openConversationSummary?.id ?? null
  const draftProjectRoot = desktopConversation.draftProjectRoot
  const draftProjectId = lockedAgentBackend ? null : (projects.find((item) => item.rootPath === draftProjectRoot)?.id ?? null)
  // External Agents use their own native permission; Fox no longer has an
  // outer full-access grant for a new conversation.
  const draftFullAccess = false
  const agentBackendFailure = useMemo(() => lastAgentBackendFailure(desktopConversation.detail), [desktopConversation.detail])
  const agentBackendBar = useMemo(() => desktopConversation.enabled ? (
    <AgentBackendPanel
      client={desktopClient}
      status={agentBackendStatus}
      lockedBackend={lockedAgentBackend}
      conversationId={openConversationId}
      draftBackend={draftAgentBackend}
      draftProjectId={draftProjectId}
      draftProjectRoot={draftProjectRoot}
      draftFullAccess={draftFullAccess}
      refusal={visibleAgentBackendRefusal}
      failure={agentBackendFailure}
    />
  ) : null, [desktopConversation.enabled, agentBackendStatus, lockedAgentBackend, openConversationId, draftAgentBackend, draftProjectId, draftProjectRoot, draftFullAccess, visibleAgentBackendRefusal, agentBackendFailure])
  const composerPanel = (
    <ComposerEntryContext.Provider value={composerEntry}>
    <Profiler id="conversation-composer" onRender={recordRegionRender}><MemoizedComposer topSlot={agentBackendBar} inputLockedReason={composerInputLockedReason} initialAttachments={composerInitialAttachments} onInitialAttachmentsConsumed={handleInitialAttachmentsConsumed} resetKey={composerResetKey} draft={composerDraft} setDraft={setComposerDraft} chatState={chatState} centered={timelineEmpty} newConversation={timelineEmpty && (!desktopConversation.enabled || !desktopConversation.selectedConversationId)} dark={dark} runtimeControlled={desktopConversation.enabled} runtimeInitializing={desktopConversation.enabled && !desktopConversation.ready && !desktopConversation.error} projectRoot={desktopConversation.detail?.conversation.projectRoot ?? desktopConversation.draftProjectRoot} projectPermissionMode={conversationAccessMode(desktopConversation.detail?.conversation) ?? activeProject?.permissionMode ?? desktopConversation.draftPermissionMode} activeAgent={activeAgent} activeExpert={activeExpert} expertReadOnly={expertReadOnly} expertToolAvailability={expertToolAvailability} agents={agentResource.agents} modelService={modelService.service} runtimeCapabilities={desktopConversation.runtimeStatus?.capabilities} yuxiModels={yuxiModels.models} goalProgressData={goalProgressData} runtimeApprovals={desktopConversation.enabled ? activeRuntimeApprovals : EMPTY_RUNTIME_APPROVALS} steeringConversationId={desktopConversation.enabled ? desktopConversation.detail?.conversation.id : undefined} steeringActive={desktopConversation.enabled && desktopBusy} onProject={handleComposerProject} onPermissionModeChange={handleComposerPermissionModeChange} onAgentChange={handleComposerAgentChange} onViewExpert={handleComposerViewExpert} onChangeExpert={handleComposerChangeExpert} onRemoveExpert={handleComposerRemoveExpert} onHeightChange={setComposerHeight} onPromptCommit={handleComposerPromptCommit} onSubmitPrompt={handleComposerSubmitPrompt} sendFailureReason={desktopConversation.lastSendFailureReason} onApprove={handleTimelineApprove} onDeny={handleTimelineDeny} onAnswer={handleTimelineAnswer} onResolveApproval={handleComposerResolveApproval} onResolveWorkModeConfirmation={handleComposerResolveWorkModeConfirmation} onResolvePlanRevision={handleComposerResolvePlanRevision} onResolveWorkflowGate={handleComposerResolveWorkflowGate} onEvidenceClick={handleComposerEvidenceClick} onStatusChange={handleComposerStatusChange} onCancel={desktopConversation.enabled ? handleComposerCancel : undefined} /></Profiler>
    </ComposerEntryContext.Provider>
  )
  const assistantUiThreadPanel = (
    <Suspense fallback={<div className="fox-app-loading" role="status" aria-label="正在加载对话" />}><div className="fox-thread-shell"><AssistantUiChat statusBar={<FoxRunStatusBar key={desktopConversation.selectedConversationId ?? 'draft'} detail={desktopConversation.enabled ? desktopConversation.detail : null} conversationId={desktopConversation.selectedConversationId} />} key={desktopConversation.detail?.conversation.id ?? 'draft'} conversation={desktopConversation} pendingMessage={pendingUserMessage} isSendDisabled={expertReadOnly || Boolean(composerInputLockedReason)} suggestions={activeAgent?.openingSuggestions} onStart={handleTimelineStart} planRevision={assistantUiPlanRevision} onOpenArtifact={handleTimelineOpenArtifact} onOpenFileInSidebar={handleTimelineOpenToolFile} onRevealFileInExplorer={handleTimelineRevealToolFile} callbacks={{ onSubmit: (prompt, files) => handleComposerSubmitPrompt(prompt, undefined, files), onOptimistic: handleComposerPromptCommit, onEdit: (messageId, prompt) => handleTimelineRerun(messageId, prompt), onReload: () => retryLatestRun() }} /></div></Suspense>
  )

  const navigationRoute = {
    view: activeView, entityId: activeEntityId, documentId: activeDocumentId,
    sourceLocator: activeSourceLocator, assistantMode,
    conversationId: activeView === 'chat' ? desktopConversation.selectedConversationId : null,
    draft: desktopConversation.captureDraft(), composerDraft, emptyConversation,
    managementReturnRoutes: managementReturnRoutes.current,
  }
  const navigationKey = JSON.stringify(activeView === 'chat'
    ? ['chat', navigationRoute.conversationId, navigationRoute.conversationId ? null : desktopConversation.draftProjectRoot]
    : [activeView, activeEntityId, activeDocumentId, activeSourceLocator])
  const navigationHistory = useNavigationHistory(activeView === 'onboarding' ? null : { key: navigationKey, route: navigationRoute }, async (route) => {
    cancelDocumentAsk()
    if (route.view === 'chat' && desktopConversation.enabled) {
      if (route.conversationId) {
        await desktopConversation.openConversation(route.conversationId)
      } else {
        desktopConversation.restoreDraft(route.draft)
      }
    }
    managementReturnRoutes.current = route.managementReturnRoutes
    setActiveView(route.view)
    setActiveEntityId(route.entityId)
    setActiveDocumentId(route.documentId)
    setActiveSourceLocator(route.sourceLocator)
    setAssistantMode(route.assistantMode)
    if (route.view === 'chat') {
      setComposerDraft(route.composerDraft)
      setEmptyConversation(route.emptyConversation)
      setPendingUserMessage(null)
    }
    setExpertDetailId(null)
    setCompactRightOpen(false)
    setRightSidebarCollapsed(true)
    setRightPanelMaximized(false)
    setRightMode(null)
  }, (error) => toast.error('无法打开历史页面', { description: desktopErrorDetails(error).message }))

  if (activeView === 'onboarding') return <Suspense fallback={<div className="fox-app-loading" role="status" aria-label="正在加载引导页" />}><OnboardingPage dark={dark} navigate={navigate} onExit={finishOnboarding} /></Suspense>

  return (
    <main className="fox-shell" style={shellStyle} onPointerDownCapture={acknowledgeVisibleCompletion} onWheelCapture={acknowledgeVisibleCompletion} onKeyDownCapture={acknowledgeVisibleCompletion}>
      <MemoizedWindowTitlebar leftSidebarCollapsed={sidebarCollapsed} onNewChat={handleNewChat} onOpenProject={handleComposerProject} onSettings={handleOpenSettings} onAbout={handleOpenAbout} onToggleSidebar={handleToggleSidebar} onZoom={handleZoom} canGoBack={navigationHistory.canGoBack} canGoForward={navigationHistory.canGoForward} onGoBack={navigationHistory.back} onGoForward={navigationHistory.forward} />
      <div className="fox-workbench">
        <MemoizedSidebar collapsed={sidebarCollapsed} assistantMode={assistantMode} onModeChange={handleAssistantModeChange} onNewChat={handleNewChat} onNewProjectChat={handleNewProjectChat} onAddProject={handleComposerProject} onCreateLocalKnowledge={handleCreateLocalKnowledge} onOpenConversation={handleOpenConversation} onRenameConversation={handleRenameConversation} projects={projects} onPinProject={handlePinProject} onPinConversation={handlePinConversation} onArchiveConversation={handleArchiveConversation} onUnarchiveConversation={handleUnarchiveConversation} onTrashConversation={handleTrashConversation} onRestoreConversation={handleRestoreConversation} onPurgeConversation={handlePurgeConversation} onDeleteProject={handleDeleteProject} onNavigate={handleWorkspaceNavigate} onExitManagement={handleExitManagement} onExpand={handleExpandSidebar} onTheme={handleToggleTheme} onSettings={handleOpenSettings} runtimeConversations={desktopConversation.enabled ? desktopConversation.conversations : undefined} archivedConversations={desktopConversation.enabled ? desktopConversation.archivedConversations : undefined} trashedConversations={desktopConversation.enabled ? desktopConversation.trashedConversations : undefined} onGlobalSearch={desktopRuntimeAvailable ? desktopClient.globalSearch : undefined} activeConversationId={desktopConversation.selectedConversationId ?? undefined} newChatActive={activeView === 'chat' && timelineEmpty && !desktopConversation.selectedConversationId} runIndicators={desktopConversation.enabled ? desktopConversation.runIndicators : undefined} yuxiService={sidebarYuxiService} yuxiUser={yuxiUser.user} activeView={activeView} activeEntityId={activeEntityId} activeDocumentId={activeDocumentId} />
        {!sidebarCollapsed && <SidebarResizeDivider width={expandedSidebarWidth} onResize={(delta) => setExpandedSidebarWidth((value) => Math.min(456, Math.max(220, value + delta)))} />}
        <div className={`fox-content-card ${showConversationRightSidebar && !rightSidebarCollapsed && (!compactLayout || compactRightOpen) ? 'has-open-right-panel' : ''}`}>
        <div className={`fox-content-surface ${rightPanelMaximized && showConversationRightSidebar && !rightSidebarCollapsed && !compactLayout ? 'is-right-maximized' : ''}`}>
        <section className="fox-chat-pane">
          {expertPickerOpen && <Suspense fallback={null}><ExpertPickerDialog open={expertPickerOpen} onOpenChange={setExpertPickerOpen} agents={agentResource.agents} defaultExpert={activeAgent} selectedExpertId={desktopConversation.selectedExpertId ?? activeAgent?.id} readOnly={expertReadOnly} onSelect={selectExpert} /></Suspense>}
          {expertDetailId && <Suspense fallback={null}><AgentDetailDialog
            agent={(() => { const record = agentResource.agents.find((item) => item.id === expertDetailId); return record ? asAgentCard(record) : null })()}
            open
            onOpenChange={(next) => { if (!next) setExpertDetailId(null) }}
            onManage={() => { const id = expertDetailId; setExpertDetailId(null); navigate('agent-detail', id, { documentId: 'home' }) }}
            onUse={() => { const id = expertDetailId; setExpertDetailId(null); void selectExpert(id) }}
          /></Suspense>}
          <TimelineRuntimeContext.Provider value={timelineRuntimeContextValue}>
          <ComposerRuntimeContext.Provider value={composerRuntimeContextValue}>
          {workspacePage ?? <>{!timelineEmpty && <ChatTopbar rightSidebarCollapsed={rightSidebarCollapsed} onRightSidebarExpand={openRightSidebarHome} onRightSidebarCollapse={collapseRightSidebar} onOpenRightMode={selectRightMode} conversation={desktopConversation.detail?.conversation} detail={desktopConversation.detail} modelName={timelineRunModel || modelService.service?.modelId || (activeAgent?.defaultModel !== 'configured-model' ? activeAgent?.defaultModel : undefined) || '未配置模型'} state={chatState} onPinConversation={(conversation) => void pinManagedConversation(conversation)} onRenameConversation={(conversation) => setConversationDialog({ conversation, mode: 'rename' })} onResetContext={(conversation) => void resetManagedConversation(conversation)} onArchiveConversation={(conversation) => void archiveManagedConversation(conversation)} />}
<div className={`fox-chat-stage ${timelineEmpty ? 'is-empty' : ''} ${activeView === 'chat' && activeExpert ? 'has-expert-lanyard' : ''}`}>
  {activeView === 'chat' && activeExpert && <Suspense fallback={null}><ExpertLanyard expertId={activeExpert.id} name={activeExpert.name} description={activeExpert.description || activeExpert.category} frontImage={asAgentCard(activeExpert).image} autoCollapseKey={timelineEmpty ? 0 : pendingUserMessage?.createdAt ?? desktopUserMessage?.createdAt ?? 1} /></Suspense>}
  {assistantUiThreadPanel}
  <DeliveryReviewPanel />
  {composerPanel}
</div></>}
          </ComposerRuntimeContext.Provider>
          </TimelineRuntimeContext.Provider>
        </section>
        {showConversationRightSidebar && !rightSidebarCollapsed && !compactLayout && <>{!rightPanelMaximized && <ResizeDivider onResize={(delta) => setRightWidth((value) => Math.min(760, Math.max(280, value + delta)))} />}<RightPanel mode={rightMode} tabs={openRightTabs} detail={desktopConversation.detail} activity={activityPanelProps} width={rightWidth} maximized={rightPanelMaximized} fileTabs={openFileTabs} activeFileTabId={activeFileTabId} activeChildAgent={activeChildAgent} onMode={selectRightMode} onCloseMode={closeRightMode} onOpenFile={openFile} onActivateFile={activateFile} onCloseFile={closeFile} onOpenChildAgent={openChildAgent} onBackChildAgent={() => setActiveChildAgent(null)} onOpenEvidence={openEvidence} onToggleMaximized={() => setRightPanelMaximized((value) => !value)} onCollapse={collapseRightSidebar} /></>}
        </div>
        {showConversationRightSidebar && !rightSidebarCollapsed && compactLayout && compactRightOpen && <CompactRightPanel maximized={rightPanelMaximized} onClose={collapseRightSidebar}>
          <RightPanel compact mode={rightMode} tabs={openRightTabs} detail={desktopConversation.detail} activity={activityPanelProps} width={rightWidth} maximized={rightPanelMaximized} fileTabs={openFileTabs} activeFileTabId={activeFileTabId} activeChildAgent={activeChildAgent} onMode={selectRightMode} onCloseMode={closeRightMode} onOpenFile={openFile} onActivateFile={activateFile} onCloseFile={closeFile} onOpenChildAgent={openChildAgent} onBackChildAgent={() => setActiveChildAgent(null)} onOpenEvidence={openEvidence} onToggleMaximized={() => setRightPanelMaximized((value) => !value)} onCollapse={collapseRightSidebar} />
        </CompactRightPanel>}
        </div>
      </div>
      <KnowledgeBindingDialog open={knowledgeDialogOpen} remoteItems={knowledge.items} localItems={localKnowledgeBases} localLoading={localKnowledgeLoading} remoteLoading={knowledge.loading} remoteError={knowledge.error} onRetry={refreshKnowledgeChoices} references={desktopConversation.knowledgeReferences} bindings={desktopConversation.knowledgeBindings} busy={knowledgeDialogBusy} error={knowledgeDialogError} onOpenChange={setKnowledgeDialogOpen} onConfirm={(references, names) => void saveKnowledgeBindings(references, names)} />
      <ConversationManagementDialogs conversation={conversationDialog?.conversation ?? null} mode={conversationDialog?.mode ?? null} busy={conversationDialogBusy} error={conversationDialogError} onClose={() => { setConversationDialog(null); setConversationDialogError(null) }} onRename={(title) => void renameManagedConversation(title)} onDelete={() => void purgeManagedConversation()} />
      <ProjectDeleteDialog project={projectDeleteDialog} busy={projectDeleteBusy} onClose={() => setProjectDeleteDialog(null)} onDelete={() => void deleteManagedProject()} />
    </main>
  )
}

export { Sidebar } from './shell/fox-sidebar'
export { RightPanel } from './right-panel/fox-right-panel'
export { KnowledgeBindingDialog } from './conversation-dialogs'
