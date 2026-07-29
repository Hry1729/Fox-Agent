import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { flushSync } from 'react-dom'
import { desktopClient, desktopRuntimeAvailable } from '../api/desktop-client'
import { mergeConversationDetail, reduceRuntimeNotifications, runRecordIsActive } from '../model/runtime-event-reducer'
import { enqueueRuntimeEvent, takeRuntimeEventFrame } from '../model/runtime-event-queue'
import { pendingRuntimeQuestion } from '../model/pending-interactions'
import { extractedAttachmentContext } from '../model/attachment-content'
import type {
  ConversationDetail,
  ConversationMessage,
  ConversationSummary,
  RunRecord,
  RuntimeEventNotification,
  RuntimeStatus,
  ProjectRecord,
  ApprovalRecord,
  KnowledgeBaseRecord,
  KnowledgeBindingRecord,
} from '../model/types'

interface DesktopConversationState {
  enabled: boolean
  ready: boolean
  conversations: ConversationSummary[]
  runtimeStatus: RuntimeStatus | null
  refreshRuntimeStatus: () => Promise<void>
  detail: ConversationDetail | null
  selectedAgentId: string | null
  draftProjectRoot: string | null
  draftPermissionMode: ProjectRecord['permissionMode'] | null
  knowledgeBindings: KnowledgeBindingRecord[]
  streamingText: string
  error: string | null
  send: (text: string, model?: string, files?: Array<{ filename?: string; mediaType?: string; url?: string }>) => Promise<boolean>
  resumeQuestion: (parentRunId: string, text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  cancel: () => Promise<void>
  resolveApproval: (approvalId: string, approved: boolean) => Promise<boolean>
  createConversation: (projectRoot?: string, permissionMode?: ProjectRecord['permissionMode']) => Promise<boolean>
  createConversationForAgent: (agentId: string, projectRoot?: string, permissionMode?: ProjectRecord['permissionMode']) => Promise<boolean>
  openConversation: (conversationId: string) => Promise<void>
  loadEarlierMessages: () => Promise<void>
  loadingEarlierMessages: boolean
  renameConversation: (conversationId: string, title: string) => Promise<boolean>
  deleteConversation: (conversationId: string) => Promise<boolean>
  searchConversations: (query: string) => Promise<ConversationSummary[]>
  setKnowledgeBindings: (knowledgeBases: KnowledgeBaseRecord[]) => Promise<boolean>
}

function runIsActive(detail: ConversationDetail | null) {
  return detail?.lastRun?.status === 'queued' || detail?.lastRun?.status === 'running' || detail?.lastRun?.status === 'cancelling'
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
  const [draftProjectRoot, setDraftProjectRoot] = useState<string | null>(null)
  const [draftPermissionMode, setDraftPermissionMode] = useState<ProjectRecord['permissionMode'] | null>(null)
  const [draftKnowledgeBases, setDraftKnowledgeBases] = useState<KnowledgeBaseRecord[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loadingEarlierMessages, setLoadingEarlierMessages] = useState(false)
  const activeConversationIdRef = useRef<string | null>(null)
  const activeRunIdRef = useRef<string | null>(null)
  const runtimeListenerReadyRef = useRef<Promise<void> | null>(null)
  const runtimeEventQueueRef = useRef<RuntimeEventNotification[]>([])
  const runtimeEventFrameRef = useRef<number | null>(null)
  const lastRuntimeEventAtRef = useRef(0)

  // Fast Refresh and draft-to-conversation transitions can preserve React state
  // while recreating refs. Keep the routing refs derived from visible state so
  // live runtime events are never filtered against a stale/null conversation.
  useLayoutEffect(() => {
    activeConversationIdRef.current = detail?.conversation.id ?? null
    const runId = detail?.lastRun?.id
    activeRunIdRef.current = runId && !runId.startsWith('pending-run-') ? runId : null
  }, [detail?.conversation.id, detail?.lastRun?.id])

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
    setDraftAgentId(next.conversation.agentId)
    setDraftProjectRoot(null)
    setDraftPermissionMode(null)
    setDraftKnowledgeBases([])
    setDetail(next)
  }, [])

  const createConversation = useCallback(async (projectRoot?: string, permissionMode?: ProjectRecord['permissionMode']) => {
    if (!defaultAgentId) return false
    const selectedAgentId = projectRoot
      ? detail?.conversation.agentId ?? draftAgentId ?? defaultAgentId
      : defaultAgentId
    setError(null)
    activeConversationIdRef.current = null
    activeRunIdRef.current = null
    setDetail(null)
    setDraftAgentId(selectedAgentId)
    setDraftProjectRoot(projectRoot?.trim() || null)
    setDraftPermissionMode(permissionMode ?? null)
    setDraftKnowledgeBases([])
    return true
  }, [defaultAgentId, detail?.conversation.agentId, draftAgentId])

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
    activeConversationIdRef.current = null
    activeRunIdRef.current = null
    setDetail(null)
    setDraftAgentId(agentId)
    setDraftProjectRoot(projectRoot?.trim() || null)
    setDraftPermissionMode(permissionMode ?? null)
    setDraftKnowledgeBases([])
    return true
  }, [])

  const renameConversation = useCallback(async (conversationId: string, title: string) => {
    try { await desktopClient.renameConversation(conversationId, title); await refreshList(); await openConversation(conversationId); return true }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
  }, [openConversation, refreshList])

  const deleteConversation = useCallback(async (conversationId: string) => {
    try {
      await desktopClient.deleteConversation(conversationId)
      await refreshList()
      if (activeConversationIdRef.current === conversationId) {
        activeConversationIdRef.current = null
        activeRunIdRef.current = null
        setDetail(null)
        setDraftAgentId(defaultAgentId)
        setDraftProjectRoot(null)
        setDraftPermissionMode(null)
        setDraftKnowledgeBases([])
      }
      return true
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); return false }
  }, [defaultAgentId, refreshList])

  const setKnowledgeBindings = useCallback(async (knowledgeBases: KnowledgeBaseRecord[]) => {
    const conversationId = activeConversationIdRef.current
    if (!conversationId) {
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
        const initialization = await desktopClient.initialize()
        if (cancelled) return
        setDefaultAgentId(initialization.defaultAgentId)
        setDraftAgentId(initialization.defaultAgentId)
        await refreshRuntimeStatus()
        await refreshList()
        if (cancelled) return
        activeConversationIdRef.current = null
        setDetail(null)
        setReady(true)
      } catch (cause) {
        if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause))
      }
    })()
    return () => { cancelled = true }
  }, [refreshList, refreshRuntimeStatus])

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
    let active = detail
    if (!active) {
      const agentId = draftAgentId ?? defaultAgentId
      if (!agentId) return false
      const conversation = await desktopClient.createConversation({
        agentId,
        projectRoot: draftProjectRoot ?? undefined,
        permissionMode: draftPermissionMode ?? undefined,
      })
      active = await desktopClient.loadConversation(conversation.id)
      activeConversationIdRef.current = conversation.id
      if (draftKnowledgeBases.length) {
        const bindings = await desktopClient.setKnowledgeBindings(
          conversation.id,
          draftKnowledgeBases.map((item) => ({ id: item.id, name: item.name })),
        )
        active = { ...active, knowledgeBindings: bindings }
      }
    }
    activeConversationIdRef.current = active.conversation.id
    const pendingRun = optimisticRun(active.conversation.id, model)
    activeRunIdRef.current = null
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
    flushSync(() => {
      setDetail((current) => {
        const base = current?.conversation.id === active.conversation.id ? current : active
        return {
          ...base,
          messages: [...base.messages, optimisticMessage],
          lastRun: pendingRun,
        }
      })
    })
    try {
      const attachmentFiles = files.flatMap((file) => file.filename && file.url?.startsWith('data:')
        ? [{ filename: file.filename, mediaType: file.mediaType, dataUrl: file.url }]
        : [])
      if (attachmentFiles.length !== files.length) {
        throw new Error('部分附件读取失败，请重新选择后再发送')
      }
      const attachments = attachmentFiles.length
        ? await desktopClient.saveAttachments(active.conversation.id, attachmentFiles)
        : []
      const isYuxi = active.conversation.agentId.startsWith('yuxi:')
      const extractedContext = !isYuxi ? await extractedAttachmentContext(files) : ''
      const attachmentNote = attachments.length && !isYuxi
        ? `\n\nFox attachments available through read_attachment:\n${attachments.map((item) => `- ${item.id}: ${item.displayName}`).join('\n')}`
        : ''
      await runtimeListenerReadyRef.current
      const started = await desktopClient.startRun({
        conversationId: active.conversation.id,
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
      setError(cause instanceof Error ? cause.message : String(cause))
      await openConversation(active.conversation.id).catch(() => undefined)
      await refreshList().catch(() => undefined)
      return false
    }
  }, [defaultAgentId, detail, draftAgentId, draftKnowledgeBases, draftPermissionMode, draftProjectRoot, openConversation, refreshList])

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

  return useMemo(() => ({
    enabled: desktopRuntimeAvailable,
    ready,
    conversations,
    runtimeStatus,
    refreshRuntimeStatus,
    detail,
    selectedAgentId: detail?.conversation.agentId ?? draftAgentId,
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
    send,
    resumeQuestion,
    cancel,
    resolveApproval,
    createConversation,
    createConversationForAgent,
    openConversation,
    loadEarlierMessages,
    loadingEarlierMessages,
    renameConversation,
    deleteConversation,
    searchConversations,
    setKnowledgeBindings,
  }), [cancel, conversations, createConversation, createConversationForAgent, deleteConversation, detail, draftAgentId, draftKnowledgeBases, draftPermissionMode, draftProjectRoot, error, loadEarlierMessages, loadingEarlierMessages, openConversation, ready, refreshRuntimeStatus, renameConversation, resolveApproval, resumeQuestion, runtimeStatus, searchConversations, send, setKnowledgeBindings])
}

export function latestMessage(messages: ConversationMessage[], role: ConversationMessage['role']) {
  return [...messages].reverse().find((message) => message.role === role)
}

export { runIsActive }
