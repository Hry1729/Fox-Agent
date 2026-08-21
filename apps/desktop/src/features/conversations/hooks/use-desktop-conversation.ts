import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { desktopClient, desktopErrorDetails, desktopRuntimeAvailable } from '../api/desktop-client'
import { applyWorkEvent, mergeConversationDetail, reduceRuntimeNotifications, runRecordIsActive } from '../model/runtime-event-reducer'
import { enqueueRuntimeEvent, takeRuntimeEventFrame } from '../model/runtime-event-queue'
import { pendingRuntimeQuestion } from '../model/pending-interactions'
import { extractedAttachmentContext } from '../model/attachment-content'
import { resolveConversationAgentId } from '../model/agent-initialization'
import { withWorkspaceInitializationTimeout } from '../model/workspace-initialization'
import type {
  ConversationDetail,
  ConversationExpertBinding,
  ConversationMessage,
  ConversationSummary,
  RunRecord,
  RuntimeEventNotification,
  RuntimeStatus,
  ProjectRecord,
  ApprovalRecord,
  KnowledgeBaseRecord,
  KnowledgeBindingRecord,
  WorkEventRecord,
  RuntimeInitialization,
  DesktopErrorDetails,
  ExpertOperationResult,
} from '../model/types'

interface ConversationDeleteResult {
  deleted: boolean
  error: string | null
}

interface DesktopConversationState {
  enabled: boolean
  ready: boolean
  conversations: ConversationSummary[]
  runtimeStatus: RuntimeStatus | null
  refreshRuntimeStatus: () => Promise<void>
  detail: ConversationDetail | null
  defaultAgentId: string | null
  selectedAgentId: string | null
  selectedExpertId: string | null
  expertBindings: ConversationExpertBinding[]
  draftProjectRoot: string | null
  draftPermissionMode: ProjectRecord['permissionMode'] | null
  knowledgeBindings: KnowledgeBindingRecord[]
  streamingText: string
  error: string | null
  errorDetails: DesktopErrorDetails | null
  send: (text: string, model?: string, files?: Array<{ filename?: string; mediaType?: string; url?: string }>) => Promise<boolean>
  rerunFromMessage: (messageId: string, text: string, model?: string) => Promise<boolean>
  resumeQuestion: (parentRunId: string, text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  cancel: () => Promise<void>
  resolveApproval: (approvalId: string, approved: boolean) => Promise<boolean>
  resolveWorkModeConfirmation: (goalId: string, expectedVersion: number, approved: boolean) => Promise<boolean>
  deleteGoal: (goalId: string) => Promise<boolean>
  setGoalRunning: (goalId: string, expectedVersion: number, running: boolean) => Promise<boolean>
  createConversation: (projectRoot?: string, permissionMode?: ProjectRecord['permissionMode']) => Promise<boolean>
  createConversationForAgent: (agentId: string, projectRoot?: string, permissionMode?: ProjectRecord['permissionMode']) => Promise<boolean>
  createConversationForExpert: (expertId: string, projectRoot?: string, permissionMode?: ProjectRecord['permissionMode']) => Promise<ExpertOperationResult>
  removeExpert: () => Promise<ExpertOperationResult>
  setDraftPermission: (permissionMode: ProjectRecord['permissionMode']) => void
  openConversation: (conversationId: string) => Promise<void>
  loadEarlierMessages: () => Promise<void>
  loadingEarlierMessages: boolean
  renameConversation: (conversationId: string, title: string) => Promise<boolean>
  setConversationPinned: (conversationId: string, pinned: boolean) => Promise<boolean>
  archiveConversation: (conversationId: string) => Promise<boolean>
  deleteConversation: (conversationId: string) => Promise<ConversationDeleteResult>
  searchConversations: (query: string) => Promise<ConversationSummary[]>
  setKnowledgeBindings: (knowledgeBases: KnowledgeBaseRecord[]) => Promise<boolean>
}

function runIsActive(detail: ConversationDetail | null) {
  return runRecordIsActive(detail?.lastRun ?? null)
}

export function activeConversationExpertBinding(detail: ConversationDetail | null) {
  return detail?.expertBindings?.find((binding) => binding.state === 'active') ?? null
}

export function conversationExpertBindingLockError(detail: ConversationDetail | null): DesktopErrorDetails | null {
  if (!detail) return null
  if (detail.conversation.archived) {
    return {
      code: 'conversation.expert_binding_locked',
      message: 'archived conversation expert binding is locked',
      retryable: false,
    }
  }
  if (runRecordIsActive(detail.lastRun)) {
    return {
      code: 'conversation.expert_binding_locked',
      message: 'conversation expert cannot change while a run is active',
      retryable: false,
    }
  }
  if (detail.messages.some((message) => message.role === 'user' || message.role === 'assistant')) {
    return {
      code: 'conversation.expert_binding_locked',
      message: 'conversation expert cannot change after user or assistant messages have been created',
      retryable: false,
    }
  }
  return null
}

export function canRemoveConversationExpert(detail: ConversationDetail | null) {
  return conversationExpertBindingLockError(detail) === null
}

function runNeedsYuxiReconciliation(detail: ConversationDetail | null) {
  const run = detail?.lastRun
  return run?.status === 'interrupted'
    && (run.errorCode === 'yuxi.connection_interrupted' || run.errorCode === 'yuxi.recovery_failed')
}

function optimisticId(prefix: string) {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2)}`
}

function optimisticRun(conversationId: string, model?: string): RunRecord {
  return {
    id: optimisticId('pending-run'),
    conversationId,
    runtimeSessionId: null,
    status: 'queued',
    model: model ?? '',
    startedAt: null,
    finishedAt: null,
    errorCode: null,
    errorMessage: null,
    lastSeq: 0,
  }
}

export function useDesktopConversation(): DesktopConversationState {
  const [ready, setReady] = useState(false)
  const [defaultAgentId, setDefaultAgentId] = useState<string | null>(null)
  const [conversations, setConversations] = useState<ConversationSummary[]>([])
  const [runtimeStatus, setRuntimeStatus] = useState<RuntimeStatus | null>(null)
  const [detail, setDetail] = useState<ConversationDetail | null>(null)
  const [draftAgentId, setDraftAgentId] = useState<string | null>(null)
  const [draftExpertId, setDraftExpertId] = useState<string | null>(null)
  const [draftProjectRoot, setDraftProjectRoot] = useState<string | null>(null)
  const [draftPermissionMode, setDraftPermissionMode] = useState<ProjectRecord['permissionMode'] | null>(null)
  const [draftKnowledgeBases, setDraftKnowledgeBases] = useState<KnowledgeBaseRecord[]>([])
  const draftRef = useRef<{
    agentId: string | null
    expertId: string | null
    projectRoot: string | null
    permissionMode: ProjectRecord['permissionMode'] | null
    knowledgeBases: KnowledgeBaseRecord[]
  }>({ agentId: null, expertId: null, projectRoot: null, permissionMode: null, knowledgeBases: [] })
  const [error, setError] = useState<string | null>(null)
  const [errorDetails, setErrorDetails] = useState<DesktopErrorDetails | null>(null)
  const [loadingEarlierMessages, setLoadingEarlierMessages] = useState(false)
  const activeConversationIdRef = useRef<string | null>(null)
  const activeRunIdRef = useRef<string | null>(null)
  const runtimeListenerReadyRef = useRef<Promise<void> | null>(null)
  const workListenerReadyRef = useRef<Promise<void> | null>(null)
  const lastWorkEventSequenceRef = useRef(new Map<string, number>())
  const runtimeEventQueueRef = useRef<RuntimeEventNotification[]>([])
  const runtimeEventFrameRef = useRef<number | null>(null)
  const lastRuntimeEventAtRef = useRef(0)
  const runtimeInitializationRef = useRef<Promise<RuntimeInitialization> | null>(null)
  const fallbackAgentId = 'fox-general'

  const applyDraft = useCallback((nextDraft: typeof draftRef.current) => {
    draftRef.current = nextDraft
    setDraftAgentId(nextDraft.agentId)
    setDraftExpertId(nextDraft.expertId)
    setDraftProjectRoot(nextDraft.projectRoot)
    setDraftPermissionMode(nextDraft.permissionMode)
    setDraftKnowledgeBases(nextDraft.knowledgeBases)
  }, [])

  const initializeRuntime = useCallback(() => {
    if (runtimeInitializationRef.current) return runtimeInitializationRef.current
    runtimeInitializationRef.current = withWorkspaceInitializationTimeout(
      desktopClient.initialize(),
      'Fox 工作区初始化',
    ).catch((cause) => {
      runtimeInitializationRef.current = null
      throw cause
    })
    return runtimeInitializationRef.current
  }, [])

  // Fast Refresh and draft-to-conversation transitions can preserve React state
  // while recreating refs. Keep the routing refs derived from visible state so
  // live runtime events are never filtered against a stale/null conversation.
  useLayoutEffect(() => {
    activeConversationIdRef.current = detail?.conversation.id ?? null
    const runId = detail?.lastRun?.id
    activeRunIdRef.current = runId && !runId.startsWith('pending-run-') ? runId : null
  }, [detail?.conversation.id, detail?.lastRun?.id])

  useLayoutEffect(() => {
    draftRef.current = {
      agentId: draftAgentId,
      expertId: draftExpertId,
      projectRoot: draftProjectRoot,
      permissionMode: draftPermissionMode,
      knowledgeBases: draftKnowledgeBases,
    }
  }, [draftAgentId, draftExpertId, draftKnowledgeBases, draftPermissionMode, draftProjectRoot])

  const refreshList = useCallback(async () => {
    const next = await desktopClient.listConversations()
    setConversations(next)
    return next
  }, [])

  const refreshRuntimeStatus = useCallback(async () => {
    if (!desktopRuntimeAvailable) return
    try { setRuntimeStatus(await desktopClient.runtimeStatus()) }
    catch { /* Runtime execution still reports actionable errors when a run starts. */ }
  }, [])

  const searchConversations = useCallback(async (query: string) => {
    if (!query.trim()) return conversations
    try {
      return await desktopClient.searchConversations(query)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return []
    }
  }, [conversations])

  const openConversation = useCallback(async (conversationId: string) => {
    const next = await desktopClient.loadConversation(conversationId)
    activeConversationIdRef.current = conversationId
    activeRunIdRef.current = runIsActive(next) ? next.lastRun?.id ?? null : null
    applyDraft({
      agentId: next.conversation.agentId,
      expertId: activeConversationExpertBinding(next)?.expertId ?? null,
      projectRoot: null,
      permissionMode: null,
      knowledgeBases: [],
    })
    setDetail(next)
    setError(null)
    setErrorDetails(null)
  }, [applyDraft])

  const createConversation = useCallback(async (projectRoot?: string, permissionMode?: ProjectRecord['permissionMode']) => {
    if (!defaultAgentId) return false
    const selectedAgentId = projectRoot
      ? detail?.conversation.agentId ?? draftAgentId ?? defaultAgentId
      : defaultAgentId
    setError(null)
    activeConversationIdRef.current = null
    activeRunIdRef.current = null
    const nextDraft = {
      agentId: selectedAgentId,
      expertId: null,
      projectRoot: projectRoot?.trim() || null,
      permissionMode: permissionMode ?? null,
      knowledgeBases: [],
    }
    applyDraft(nextDraft)
    setDetail(null)
    return true
  }, [applyDraft, defaultAgentId, detail?.conversation.agentId, draftAgentId])

  const loadEarlierMessages = useCallback(async () => {
    const current = detail
    const oldestOrdinal = current?.messages[0]?.ordinal
    if (!current?.hasEarlierMessages || oldestOrdinal == null || loadingEarlierMessages) return
    setLoadingEarlierMessages(true)
    try {
      const page = await desktopClient.loadConversationHistory(current.conversation.id, oldestOrdinal)
      setDetail((value) => {
        if (!value || value.conversation.id !== current.conversation.id) return value
        const merge = <T,>(older: T[], existing: T[], key: (item: T) => string) => {
          const records = new Map(older.map((item) => [key(item), item]))
          existing.forEach((item) => records.set(key(item), item))
          return [...records.values()]
        }
        return {
          ...value,
          messages: merge(page.messages, value.messages, (item) => item.id).sort((a, b) => a.ordinal - b.ordinal),
          runtimeEvents: merge(page.runtimeEvents, value.runtimeEvents, (item) => `${item.runId}:${item.seq}`).sort((a, b) => a.createdAt - b.createdAt || a.seq - b.seq),
          toolCalls: merge(page.toolCalls, value.toolCalls, (item) => item.id),
          approvals: merge(page.approvals, value.approvals, (item) => item.id),
          attachments: merge(page.attachments, value.attachments, (item) => item.id),
          artifacts: merge(page.artifacts, value.artifacts, (item) => item.id),
          hasEarlierMessages: page.hasEarlierMessages,
        }
      })
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoadingEarlierMessages(false) }
  }, [detail, loadingEarlierMessages])

  const createConversationForAgent = useCallback(async (agentId: string, projectRoot?: string, permissionMode?: ProjectRecord['permissionMode']) => {
    setError(null)
    setErrorDetails(null)
    activeConversationIdRef.current = null
    activeRunIdRef.current = null
    const nextDraft = {
      agentId,
      expertId: null,
      projectRoot: projectRoot?.trim() || null,
      permissionMode: permissionMode ?? null,
      knowledgeBases: [],
    }
    applyDraft(nextDraft)
    setDetail(null)
    return true
  }, [applyDraft])

  const createConversationForExpert = useCallback(async (
    expertId: string,
    projectRoot?: string,
    permissionMode?: ProjectRecord['permissionMode'],
  ) => {
    const selectedExpertId = expertId.trim()
    if (!selectedExpertId) {
      const invalid = { code: 'expert.invalid', message: 'expert id is required', retryable: false }
      setError(invalid.message)
      setErrorDetails(invalid)
      return { success: false, error: invalid }
    }
    try {
      const resolvedAgent = await resolveConversationAgentId([defaultAgentId], initializeRuntime)
      if (resolvedAgent.initialized) setDefaultAgentId(resolvedAgent.agentId)
      setError(null)
      setErrorDetails(null)
      activeConversationIdRef.current = null
      activeRunIdRef.current = null
      applyDraft({
        agentId: resolvedAgent.agentId,
        expertId: selectedExpertId,
        projectRoot: projectRoot?.trim() || null,
        permissionMode: permissionMode ?? null,
        knowledgeBases: [],
      })
      setDetail(null)
      return { success: true, error: null }
    } catch (cause) {
      const details = desktopErrorDetails(cause)
      setError(details.message)
      setErrorDetails(details)
      return { success: false, error: details }
    }
  }, [applyDraft, defaultAgentId, initializeRuntime])

  const setDraftPermission = useCallback((permissionMode: ProjectRecord['permissionMode']) => {
    draftRef.current = { ...draftRef.current, permissionMode }
    setDraftPermissionMode(permissionMode)
  }, [])

  const renameConversation = useCallback(async (conversationId: string, title: string) => {
    try { await desktopClient.renameConversation(conversationId, title); await refreshList(); await openConversation(conversationId); return true }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
  }, [openConversation, refreshList])

  const setConversationPinned = useCallback(async (conversationId: string, pinned: boolean) => {
    try {
      const updated = await desktopClient.setConversationPinned(conversationId, pinned)
      await refreshList()
      setDetail((current) => current?.conversation.id === conversationId
        ? { ...current, conversation: updated }
        : current)
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
  }, [refreshList])

  const archiveConversation = useCallback(async (conversationId: string) => {
    try {
      await desktopClient.archiveConversation(conversationId)
      await refreshList()
      if (activeConversationIdRef.current === conversationId) {
        activeConversationIdRef.current = null
        activeRunIdRef.current = null
        applyDraft({ agentId: defaultAgentId, expertId: null, projectRoot: null, permissionMode: null, knowledgeBases: [] })
        setDetail(null)
      }
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
  }, [applyDraft, defaultAgentId, refreshList])

  const deleteConversation = useCallback(async (conversationId: string) => {
    try {
      await desktopClient.deleteConversation(conversationId)
      await refreshList()
      if (activeConversationIdRef.current === conversationId) {
        activeConversationIdRef.current = null
        activeRunIdRef.current = null
        applyDraft({ agentId: defaultAgentId, expertId: null, projectRoot: null, permissionMode: null, knowledgeBases: [] })
        setDetail(null)
      }
      return { deleted: true, error: null }
    } catch (cause) {
      return {
        deleted: false,
        error: cause instanceof Error ? cause.message : String(cause),
      }
    }
  }, [applyDraft, defaultAgentId, refreshList])

  const removeExpert = useCallback(async () => {
    const current = detail
    if (!current) {
      applyDraft({ ...draftRef.current, expertId: null })
      setError(null)
      setErrorDetails(null)
      return { success: true, error: null }
    }
    const lockError = conversationExpertBindingLockError(current)
    if (lockError) {
      setError(lockError.message)
      setErrorDetails(lockError)
      return { success: false, error: lockError }
    }
    if (!activeConversationExpertBinding(current)) {
      applyDraft({ ...draftRef.current, expertId: null })
      setError(null)
      setErrorDetails(null)
      return { success: true, error: null }
    }
    try {
      await desktopClient.removeConversationExpert(current.conversation.id)
      await openConversation(current.conversation.id)
      setErrorDetails(null)
      return { success: true, error: null }
    } catch (cause) {
      const details = desktopErrorDetails(cause)
      setError(details.message)
      setErrorDetails(details)
      return { success: false, error: details }
    }
  }, [applyDraft, detail, openConversation])

  const setKnowledgeBindings = useCallback(async (knowledgeBases: KnowledgeBaseRecord[]) => {
    const conversationId = activeConversationIdRef.current
    if (!conversationId) {
      draftRef.current = { ...draftRef.current, knowledgeBases }
      setDraftKnowledgeBases(knowledgeBases)
      setError(null)
      return true
    }
    try {
      const bindings = await desktopClient.setKnowledgeBindings(
        conversationId,
        knowledgeBases.map((item) => ({ id: item.id, name: item.name })),
      )
      setDetail((current) => current && current.conversation.id === conversationId
        ? { ...current, knowledgeBindings: bindings }
        : current)
      setError(null)
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
  }, [])

  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    let cancelled = false
    void (async () => {
      try {
        const { agentId } = await resolveConversationAgentId([], initializeRuntime)
        if (cancelled) return
        setDefaultAgentId(agentId)
        applyDraft({
          ...draftRef.current,
          agentId: draftRef.current.agentId ?? agentId,
          expertId: null,
        })
        await Promise.all([
          withWorkspaceInitializationTimeout(refreshRuntimeStatus(), '运行状态加载'),
          withWorkspaceInitializationTimeout(refreshList(), '对话列表加载'),
        ])
        if (cancelled) return
        activeConversationIdRef.current = null
        setDetail(null)
        setError(null)
        setReady(true)
      } catch (cause) {
        if (!cancelled) {
          setDefaultAgentId((current) => current ?? fallbackAgentId)
          applyDraft({
            ...draftRef.current,
            agentId: draftRef.current.agentId ?? fallbackAgentId,
            expertId: null,
          })
          setError(cause instanceof Error ? cause.message : String(cause))
        }
      } finally {
        if (!cancelled) setReady(true)
      }
    })()
    return () => { cancelled = true }
  }, [applyDraft, initializeRuntime, refreshList, refreshRuntimeStatus])

  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    let disposed = false
    const stops: Array<() => void> = []
    const updateApproval = (approval: ApprovalRecord) => {
      if (disposed || approval.conversationId !== activeConversationIdRef.current) return
      setDetail((current) => {
        if (!current) return current
        const exists = current.approvals.some((item) => item.id === approval.id)
        return {
          ...current,
          approvals: exists
            ? current.approvals.map((item) => item.id === approval.id ? approval : item)
            : [...current.approvals, approval],
        }
      })
    }
    void desktopClient.listenApprovalRequests(updateApproval).then((stop) => disposed ? stop() : stops.push(stop))
    void desktopClient.listenApprovalResolved(updateApproval).then((stop) => disposed ? stop() : stops.push(stop))
    return () => {
      disposed = true
      stops.forEach((stop) => stop())
    }
  }, [])

  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    let disposed = false
    let unlisten: (() => void) | undefined
    const flushRuntimeEvents = () => {
      runtimeEventFrameRef.current = null
      if (disposed || runtimeEventQueueRef.current.length === 0) return
      const notifications = takeRuntimeEventFrame(runtimeEventQueueRef.current)
      flushSync(() => {
        setDetail((current) => current ? reduceRuntimeNotifications(current, notifications) : current)
      })
      const terminal = [...notifications].reverse().find(({ event }) => (
        event.type === 'run.completed'
        || event.type === 'run.cancelled'
        || event.type === 'run.failed'
        || event.type === 'run.interrupted'
      ))
      if (runtimeEventQueueRef.current.length > 0) {
        scheduleRuntimeFlush()
      }
      if (!terminal) return
      const event = terminal.event
      const isCurrentConversation = activeConversationIdRef.current === terminal.conversationId
      if (isCurrentConversation && (event.type === 'run.failed' || event.type === 'run.interrupted') && typeof event.message === 'string') {
        setError(event.message)
        setErrorDetails({
          code: typeof event.code === 'string' ? event.code : 'runtime.failed',
          message: event.message,
          retryable: event.type === 'run.interrupted',
        })
      }
      if (!isCurrentConversation) {
        void refreshList()
        return
      }
      void desktopClient.loadConversation(terminal.conversationId).then((persisted) => {
        if (disposed || activeConversationIdRef.current !== terminal.conversationId) return
        flushSync(() => setDetail((current) => mergeConversationDetail(persisted, current)))
      }).catch(() => undefined)
      void refreshList()
    }
    const scheduleRuntimeFlush = () => {
      if (runtimeEventFrameRef.current !== null) return
      runtimeEventFrameRef.current = window.requestAnimationFrame(flushRuntimeEvents)
    }
    const subscription = desktopClient.listenRuntimeEvents((notification: RuntimeEventNotification) => {
      const decision = disposed ? 'disposed' : 'accepted'
      if (decision !== 'accepted') return
      const event = notification.event
      if (event.type === 'run.started') {
        setError(null)
        setErrorDetails(null)
      }
      lastRuntimeEventAtRef.current = Date.now()
      enqueueRuntimeEvent(runtimeEventQueueRef.current, notification)
      scheduleRuntimeFlush()
    })
    runtimeListenerReadyRef.current = subscription.then((stop) => {
      if (disposed) stop()
      else unlisten = stop
    }).catch((cause) => {
      if (!disposed) {
        const message = cause instanceof Error ? cause.message : String(cause)
        setError(message)
      }
    })
    return () => {
      disposed = true
      runtimeEventQueueRef.current = []
      lastRuntimeEventAtRef.current = 0
      if (runtimeEventFrameRef.current !== null) {
        window.cancelAnimationFrame(runtimeEventFrameRef.current)
        runtimeEventFrameRef.current = null
      }
      unlisten?.()
      runtimeListenerReadyRef.current = null
    }
  }, [refreshList])

  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    let disposed = false
    let unlisten: (() => void) | undefined
    const subscription = desktopClient.listenWorkEvents((event: WorkEventRecord) => {
      if (disposed) return
      const previous = lastWorkEventSequenceRef.current.get(event.conversationId) ?? -1
      if (event.sequence <= previous) return
      lastWorkEventSequenceRef.current.set(event.conversationId, event.sequence)
      setDetail((current) => applyWorkEvent(current, event))
    })
    workListenerReadyRef.current = subscription.then((stop) => {
      if (disposed) stop()
      else unlisten = stop
    }).catch((cause) => {
      if (!disposed) setError(cause instanceof Error ? cause.message : String(cause))
    })
    return () => {
      disposed = true
      unlisten?.()
      workListenerReadyRef.current = null
      lastWorkEventSequenceRef.current.clear()
    }
  }, [])

  const activeConversationId = detail?.conversation.id ?? null
  const activeRunId = detail?.lastRun?.id ?? null
  const activeRunStatus = detail?.lastRun?.status ?? null
  useEffect(() => {
    if (!desktopRuntimeAvailable || !activeConversationId || !activeRunId) return
    const recoverableYuxiRun = runNeedsYuxiReconciliation(detail)
    if (activeRunId.startsWith('pending-run-') || (!['queued', 'running', 'cancelling'].includes(activeRunStatus ?? '') && !recoverableYuxiRun)) return

    let disposed = false
    let loading = false
    let timer: number | null = null
    let delayMs = 750
    let keepPolling = true
    let lastFingerprint = `${activeRunStatus}:${detail?.lastRun?.lastSeq ?? 0}:${detail?.messages.length ?? 0}`
    const schedule = (delay = delayMs) => {
      if (disposed || !keepPolling) return
      if (timer !== null) window.clearTimeout(timer)
      timer = window.setTimeout(() => void synchronize(), delay)
    }
    const synchronize = async () => {
      if (disposed || loading) return
      if (runtimeEventQueueRef.current.length > 0 || Date.now() - lastRuntimeEventAtRef.current < 1_500) {
        delayMs = 750
        schedule()
        return
      }
      loading = true
      const lastRuntimeEventAt = lastRuntimeEventAtRef.current
      try {
        const persisted = await desktopClient.loadConversation(activeConversationId)
        if (disposed) return
        if (runtimeEventQueueRef.current.length > 0 || lastRuntimeEventAtRef.current !== lastRuntimeEventAt) return
        if (persisted.lastRun?.id !== activeRunId) {
          keepPolling = false
          activeRunIdRef.current = runRecordIsActive(persisted.lastRun) ? persisted.lastRun?.id ?? null : null
          setDetail((current) => mergeConversationDetail(persisted, current))
          void refreshList()
          return
        }
        const fingerprint = `${persisted.lastRun.status}:${persisted.lastRun.lastSeq}:${persisted.messages.length}`
        const changed = fingerprint !== lastFingerprint
        lastFingerprint = fingerprint
        delayMs = changed ? 750 : Math.min(3_000, Math.round(delayMs * 1.6))
        setDetail((current) => mergeConversationDetail(persisted, current))
        if (!runRecordIsActive(persisted.lastRun)) {
          keepPolling = false
          activeRunIdRef.current = null
          void refreshList()
        }
      } catch {
        delayMs = Math.min(3_000, Math.round(delayMs * 1.6))
      } finally {
        loading = false
        schedule()
      }
    }

    void synchronize()
    return () => {
      disposed = true
      if (timer !== null) window.clearTimeout(timer)
    }
  }, [activeConversationId, activeRunId, activeRunStatus, detail?.lastRun?.errorCode, refreshList])

  const send = useCallback(async (text: string, model?: string, files: Array<{ filename?: string; mediaType?: string; url?: string }> = []) => {
    const cleanText = text.trim()
    if (!cleanText) return false
    setError(null)
    setErrorDetails(null)
    let active = detail
    try {
      if (!active) {
        const draft = draftRef.current
        const resolvedAgent = await resolveConversationAgentId(
          [draft.agentId, draftAgentId, defaultAgentId],
          initializeRuntime,
        )
        const agentId = resolvedAgent.agentId
        if (resolvedAgent.initialized) {
          setDefaultAgentId(agentId)
          applyDraft({ ...draft, agentId })
        }
        const conversation = await desktopClient.createConversation({
          agentId,
          expertId: draft.expertId ?? draftExpertId ?? undefined,
          projectRoot: draft.projectRoot ?? draftProjectRoot ?? undefined,
          permissionMode: draft.permissionMode ?? draftPermissionMode ?? undefined,
        })
        active = await desktopClient.loadConversation(conversation.id)
        activeConversationIdRef.current = conversation.id
        const knowledgeBases = draft.knowledgeBases.length ? draft.knowledgeBases : draftKnowledgeBases
        if (knowledgeBases.length) {
          const bindings = await desktopClient.setKnowledgeBindings(
            conversation.id,
            knowledgeBases.map((item) => ({ id: item.id, name: item.name })),
          )
          active = { ...active, knowledgeBindings: bindings }
        }
        applyDraft({
          agentId: active.conversation.agentId,
          expertId: activeConversationExpertBinding(active)?.expertId ?? draft.expertId ?? draftExpertId,
          projectRoot: null,
          permissionMode: null,
          knowledgeBases: [],
        })
      }
      const activeConversation = active
      activeConversationIdRef.current = activeConversation.conversation.id
      const pendingRun = optimisticRun(activeConversation.conversation.id, model)
      activeRunIdRef.current = null
      const pendingMessageId = optimisticId('pending-message')
      const now = Date.now()
      const nextOrdinal = activeConversation.messages.reduce((highest, message) => Math.max(highest, message.ordinal), 0) + 1
      const optimisticMessage: ConversationMessage = {
        id: pendingMessageId,
        conversationId: activeConversation.conversation.id,
        runId: pendingRun.id,
        role: 'user',
        kind: 'text',
        content: cleanText,
        status: 'sending',
        ordinal: nextOrdinal,
        createdAt: now,
        updatedAt: now,
      }
      flushSync(() => {
        setDetail((current) => {
          const base = current?.conversation.id === activeConversation.conversation.id ? current : activeConversation
          return {
            ...base,
            messages: [...base.messages, optimisticMessage],
            lastRun: pendingRun,
          }
        })
      })
      const attachmentFiles = files.flatMap((file) => file.filename && file.url?.startsWith('data:')
        ? [{ filename: file.filename, mediaType: file.mediaType, dataUrl: file.url }]
        : [])
      if (attachmentFiles.length !== files.length) {
        throw new Error('部分附件读取失败，请重新选择后再发送')
      }
      const attachments = attachmentFiles.length
        ? await desktopClient.saveAttachments(activeConversation.conversation.id, attachmentFiles)
        : []
      const isYuxi = activeConversation.conversation.agentId.startsWith('yuxi:')
      const extractedContext = !isYuxi ? await extractedAttachmentContext(files) : ''
      const attachmentNote = attachments.length && !isYuxi
        ? `\n\nFox attachments available through read_attachment:\n${attachments.map((item) => `- ${item.id}: ${item.displayName}`).join('\n')}`
        : ''
      await Promise.all([runtimeListenerReadyRef.current, workListenerReadyRef.current])
      const started = await desktopClient.startRun({
        conversationId: activeConversation.conversation.id,
        text: cleanText,
        runtimeText: `${cleanText}${attachmentNote}${extractedContext}`,
        model,
        attachmentIds: attachments.map((item) => item.id),
      })
      activeRunIdRef.current = started.run.id
      void desktopClient.runtimeStatus().then(setRuntimeStatus).catch(() => undefined)
      setDetail((current) => {
        if (!current) return current
        const runState = current.lastRun?.id === started.run.id ? current.lastRun : null
        const pendingState = current.lastRun?.id.startsWith('pending-run-') ? current.lastRun : null
        return {
          ...current,
          messages: current.messages.map((message) => message.id === pendingMessageId ? started.userMessage : message),
          attachments: [
            ...current.attachments.filter((item) => !started.attachments.some((attachment) => attachment.id === item.id)),
            ...started.attachments,
          ],
          lastRun: runState
            ? {
                ...started.run,
                status: runState.status,
                startedAt: runState.startedAt ?? started.run.startedAt,
                finishedAt: runState.finishedAt,
                errorCode: runState.errorCode,
                errorMessage: runState.errorMessage,
                lastSeq: Math.max(runState.lastSeq, started.run.lastSeq),
              }
            : pendingState
              ? started.run
              : current.lastRun ?? started.run,
        }
      })
      await refreshList()
      return true
    } catch (cause) {
      const details = desktopErrorDetails(cause)
      if (active) await openConversation(active.conversation.id).catch(() => undefined)
      await refreshList().catch(() => undefined)
      setError(details.message)
      setErrorDetails(details)
      return false
    }
  }, [applyDraft, defaultAgentId, detail, draftAgentId, draftExpertId, draftKnowledgeBases, draftPermissionMode, draftProjectRoot, initializeRuntime, openConversation, refreshList])

  const rerunFromMessage = useCallback(async (messageId: string, text: string, model?: string) => {
    const active = detail
    const cleanText = text.trim()
    if (!active || !cleanText || runIsActive(active)) return false
    const target = active.messages.find((message) => message.id === messageId && message.role === 'user')
    if (!target) return false
    setError(null)
    const pendingRun = optimisticRun(active.conversation.id, model)
    const pendingMessageId = optimisticId('pending-message')
    const affectedRunIds = new Set(active.messages
      .filter((message) => message.ordinal >= target.ordinal && message.runId)
      .map((message) => message.runId as string))
    const optimisticMessage: ConversationMessage = {
      ...target,
      id: pendingMessageId,
      runId: pendingRun.id,
      content: cleanText,
      status: 'sending',
      updatedAt: Date.now(),
    }
    flushSync(() => {
      setDetail((current) => {
        if (!current || current.conversation.id !== active.conversation.id) return current
        return {
          ...current,
          messages: [
            ...current.messages.filter((message) => message.ordinal < target.ordinal),
            optimisticMessage,
          ],
          runtimeEvents: current.runtimeEvents.filter((event) => !affectedRunIds.has(event.runId)),
          toolCalls: current.toolCalls.filter((toolCall) => !affectedRunIds.has(toolCall.runId)),
          approvals: current.approvals.filter((approval) => !affectedRunIds.has(approval.runId)),
          artifacts: current.artifacts.filter((artifact) => !artifact.runId || !affectedRunIds.has(artifact.runId)),
          attachments: current.attachments.map((attachment) => attachment.messageId === messageId
            ? { ...attachment, messageId: pendingMessageId }
            : attachment),
          lastRun: pendingRun,
        }
      })
    })
    try {
      await Promise.all([runtimeListenerReadyRef.current, workListenerReadyRef.current])
      const started = await desktopClient.rewindRun({
        conversationId: active.conversation.id,
        messageId,
        text: cleanText,
        model,
      })
      activeRunIdRef.current = started.run.id
      const persisted = await desktopClient.loadConversation(active.conversation.id)
      setDetail(persisted)
      void desktopClient.runtimeStatus().then(setRuntimeStatus).catch(() => undefined)
      void refreshList()
      return true
    } catch (cause) {
      const details = desktopErrorDetails(cause)
      await openConversation(active.conversation.id).catch(() => undefined)
      setError(details.message)
      setErrorDetails(details)
      return false
    }
  }, [detail, openConversation, refreshList])

  const resumeQuestion = useCallback(async (
    parentRunId: string,
    text: string,
    answers: Record<string, string | string[]>,
  ) => {
    const active = detail
    const cleanText = text.trim()
    if (!active || !cleanText) return false
    if (pendingRuntimeQuestion(active)?.runId !== parentRunId) return false
    setError(null)
    const pendingRun = optimisticRun(active.conversation.id)
    const pendingMessageId = optimisticId('pending-message')
    const now = Date.now()
    const nextOrdinal = active.messages.reduce((highest, message) => Math.max(highest, message.ordinal), 0) + 1
    const optimisticMessage: ConversationMessage = {
      id: pendingMessageId,
      conversationId: active.conversation.id,
      runId: pendingRun.id,
      role: 'user',
      kind: 'text',
      content: cleanText,
      status: 'sending',
      ordinal: nextOrdinal,
      createdAt: now,
      updatedAt: now,
    }
    const parentLastSeq = active.runtimeEvents.reduce((highest, item) => (
      item.runId === parentRunId ? Math.max(highest, item.seq) : highest
    ), 0)
    const respondedEvent = {
      runId: parentRunId,
      seq: parentLastSeq + 1,
      eventType: 'user.question.responded',
      event: { type: 'user.question.responded', childRunId: pendingRun.id },
      createdAt: now,
    }
    activeRunIdRef.current = null
    flushSync(() => setDetail((current) => current?.conversation.id === active.conversation.id
      ? { ...current, messages: [...current.messages, optimisticMessage], runtimeEvents: [...current.runtimeEvents, respondedEvent], lastRun: pendingRun }
      : current))
    try {
      await runtimeListenerReadyRef.current
      const started = await desktopClient.resumeRun({
        conversationId: active.conversation.id,
        parentRunId,
        text: cleanText,
        answers,
      })
      activeRunIdRef.current = started.run.id
      setDetail((current) => {
        if (!current) return current
        const runState = current.lastRun?.id === started.run.id ? current.lastRun : null
        const pendingState = current.lastRun?.id.startsWith('pending-run-') ? current.lastRun : null
        return {
          ...current,
          messages: current.messages.map((message) => message.id === pendingMessageId ? started.userMessage : message),
          lastRun: runState
            ? {
                ...started.run,
                status: runState.status,
                startedAt: runState.startedAt ?? started.run.startedAt,
                finishedAt: runState.finishedAt,
                errorCode: runState.errorCode,
                errorMessage: runState.errorMessage,
                lastSeq: Math.max(runState.lastSeq, started.run.lastSeq),
              }
            : pendingState ? started.run : current.lastRun ?? started.run,
        }
      })
      await refreshList()
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      await openConversation(active.conversation.id).catch(() => undefined)
      await refreshList().catch(() => undefined)
      return false
    }
  }, [detail, openConversation, refreshList])

  const cancel = useCallback(async () => {
    const runId = detail?.lastRun?.id
    if (!runId) return
    await desktopClient.cancelRun(runId)
  }, [detail?.lastRun?.id])

  const resolveApproval = useCallback(async (approvalId: string, approved: boolean) => {
    const resolvedAt = Date.now()
    let previous: ApprovalRecord | undefined
    setDetail((current) => {
      if (!current) return current
      previous = current.approvals.find((item) => item.id === approvalId)
      if (!previous || previous.status !== 'pending') return current
      return {
        ...current,
        approvals: current.approvals.map((item) => item.id === approvalId ? {
          ...item,
          status: approved ? 'approved' : 'denied',
          decision: { approved },
          resolvedAt,
        } : item),
      }
    })
    try {
      const resolved = await desktopClient.resolveApproval(approvalId, approved)
      if (!resolved) throw new Error('这个审批已经处理或失效')
      const conversationId = activeConversationIdRef.current
      if (conversationId) {
        void desktopClient.loadConversation(conversationId).then((persisted) => {
          setDetail((current) => mergeConversationDetail(persisted, current))
        }).catch(() => undefined)
      }
      return true
    } catch (cause) {
      if (previous) {
        setDetail((current) => current ? {
          ...current,
          approvals: current.approvals.map((item) => item.id === approvalId ? previous! : item),
        } : current)
      }
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
  }, [])

  const resolveWorkModeConfirmation = useCallback(async (
    goalId: string,
    expectedVersion: number,
    approved: boolean,
  ) => {
    const conversationId = activeConversationIdRef.current
    if (!conversationId) return false
    try {
      await desktopClient.resolveWorkModeConfirmation(conversationId, goalId, expectedVersion, approved)
      const persisted = await desktopClient.loadConversation(conversationId)
      activeRunIdRef.current = runRecordIsActive(persisted.lastRun) ? persisted.lastRun?.id ?? null : null
      setDetail((current) => mergeConversationDetail(persisted, current))
      setError(null)
      void refreshList()
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
  }, [refreshList])

  const deleteGoal = useCallback(async (goalId: string) => {
    const conversationId = activeConversationIdRef.current
    if (!conversationId) return false
    try {
      const deleted = await desktopClient.deleteGoal(conversationId, goalId)
      if (!deleted) return false
      setDetail((current) => {
        if (!current || current.conversation.id !== conversationId) return current
        const taskIds = new Set(current.tasks.filter((task) => task.goalId === goalId).map((task) => task.id))
        return {
          ...current,
          goals: current.goals.filter((goal) => goal.id !== goalId),
          tasks: current.tasks.filter((task) => task.goalId !== goalId),
          evidence: current.evidence.filter((item) => !taskIds.has(item.taskId)),
        }
      })
      const persisted = await desktopClient.loadConversation(conversationId)
      activeRunIdRef.current = runRecordIsActive(persisted.lastRun) ? persisted.lastRun?.id ?? null : null
      setDetail((current) => current?.conversation.id === conversationId ? persisted : current)
      setError(null)
      void refreshList()
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
  }, [refreshList])

  const setGoalRunning = useCallback(async (goalId: string, expectedVersion: number, running: boolean) => {
    const conversationId = activeConversationIdRef.current
    if (!conversationId) return false
    try {
      const result = await desktopClient.setGoalRunning(conversationId, goalId, expectedVersion, running)
      activeRunIdRef.current = running ? result.startedRun?.run.id ?? null : null
      const persisted = await desktopClient.loadConversation(conversationId)
      setDetail((current) => mergeConversationDetail(persisted, current))
      setError(null)
      return true
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
      return false
    }
  }, [])

  return useMemo(() => ({
    enabled: desktopRuntimeAvailable,
    ready,
    conversations,
    runtimeStatus,
    refreshRuntimeStatus,
    detail,
    defaultAgentId,
    selectedAgentId: detail?.conversation.agentId ?? draftAgentId,
    selectedExpertId: detail ? activeConversationExpertBinding(detail)?.expertId ?? null : draftExpertId,
    expertBindings: detail?.expertBindings ?? [],
    draftProjectRoot,
    draftPermissionMode,
    knowledgeBindings: detail?.knowledgeBindings ?? draftKnowledgeBases.map((item) => ({
      conversationId: 'draft',
      serviceConnectionId: 'yuxi',
      knowledgeBaseId: item.id,
      knowledgeBaseName: item.name,
      enabled: true,
      createdAt: 0,
      updatedAt: 0,
    })),
    streamingText: detail?.lastRun
      ? [...detail.messages].reverse().find((message) => message.role === 'assistant' && message.runId === detail.lastRun?.id)?.content ?? ''
      : '',
    error,
    errorDetails,
    send,
    rerunFromMessage,
    resumeQuestion,
    cancel,
    resolveApproval,
    resolveWorkModeConfirmation,
    deleteGoal,
    setGoalRunning,
    createConversation,
    createConversationForAgent,
    createConversationForExpert,
    removeExpert,
    setDraftPermission,
    openConversation,
    loadEarlierMessages,
    loadingEarlierMessages,
    renameConversation,
    setConversationPinned,
    archiveConversation,
    deleteConversation,
    searchConversations,
    setKnowledgeBindings,
  }), [archiveConversation, cancel, conversations, createConversation, createConversationForAgent, createConversationForExpert, defaultAgentId, deleteConversation, deleteGoal, detail, draftAgentId, draftExpertId, draftKnowledgeBases, draftPermissionMode, draftProjectRoot, error, errorDetails, loadEarlierMessages, loadingEarlierMessages, openConversation, ready, refreshRuntimeStatus, removeExpert, renameConversation, rerunFromMessage, resolveApproval, resolveWorkModeConfirmation, resumeQuestion, runtimeStatus, searchConversations, send, setConversationPinned, setDraftPermission, setGoalRunning, setKnowledgeBindings])
}

export function latestMessage(messages: ConversationMessage[], role: ConversationMessage['role']) {
  return [...messages].reverse().find((message) => message.role === role)
}

export { runIsActive }
