import type {
  ConversationDetail,
  ConversationMessage,
  GoalRecord,
  WorkTaskRecord,
  TaskEvidenceRecord,
  PlanRevisionRecord,
  ReviewFindingRecord,
  AcceptanceRecord,
  RunRecord,
  RuntimeEventNotification,
  WorkEventRecord,
} from './types'
import { conversationRunIsActive, mergeKernelSnapshot, snapshotForRun } from './kernel-snapshot'
import { answerDeltaFingerprint } from './runtime-delta-fingerprint'
import { previewBelongsToSnapshot } from './kernel-model-preview'

export function runRecordIsActive(run: RunRecord | null) {
  return run?.status === 'queued' || run?.status === 'running' || run?.status === 'cancelling'
}

function eventRun(current: RunRecord | null, notification: RuntimeEventNotification): RunRecord {
  const event = notification.event
  const successfulLifecycle = event.type === 'run.started' || event.type === 'run.completed'
  const terminal = event.type === 'run.completed' || event.type === 'run.cancelled' || event.type === 'run.failed' || event.type === 'run.interrupted'
  const status = event.type === 'run.completed'
    ? 'completed'
    : event.type === 'run.cancelled'
      ? 'cancelled'
      : event.type === 'run.failed'
        ? 'failed'
        : event.type === 'run.interrupted'
          ? 'interrupted'
          : 'running'
  const previous = current?.id === notification.runId ? current : null
  return {
    id: notification.runId,
    conversationId: notification.conversationId,
    runtimeSessionId: notification.runtimeSessionId ?? previous?.runtimeSessionId ?? null,
    status,
    model: typeof event.model === 'string' ? event.model : previous?.model ?? '',
    startedAt: previous?.startedAt ?? Date.parse(notification.timestamp),
    finishedAt: terminal ? Date.parse(notification.timestamp) : null,
    errorCode: successfulLifecycle
      ? null
      : (event.type === 'run.failed' || event.type === 'run.interrupted') && typeof event.code === 'string'
        ? event.code
        : previous?.errorCode ?? null,
    errorMessage: successfulLifecycle
      ? null
      : (event.type === 'run.failed' || event.type === 'run.interrupted') && typeof event.message === 'string'
        ? event.message
        : previous?.errorMessage ?? null,
    lastSeq: Math.max(previous?.lastSeq ?? 0, notification.seq),
  }
}

function mergeRecords<T>(persisted: T[], current: T[], key: (item: T) => string) {
  const records = new Map<string, T>()
  current.forEach((item) => records.set(key(item), item))
  persisted.forEach((item) => records.set(key(item), item))
  return [...records.values()]
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}

function mergeMessages(persisted: ConversationMessage[], current: ConversationMessage[]) {
  const persistedMessages = new Map(persisted.map((message) => [message.id, message]))
  const hasPersistedEquivalent = (message: ConversationMessage) => persisted.some((candidate) => (
    candidate.role === message.role
    && candidate.content === message.content
    && Math.abs(candidate.createdAt - message.createdAt) < 10_000
  ))

  for (const message of current) {
    const stored = persistedMessages.get(message.id)
    if (stored) {
      if (stored.id.startsWith('kernel-message:')) {
        if (stored.status === 'streaming' && message.updatedAt >= stored.updatedAt && message.kernelPreview) {
          persistedMessages.set(message.id, message)
        }
        // A Kernel display frame replaces the whole prior frame, even when it
        // is shorter. Never combine one revision's text with another's cursor.
        continue
      }
      const statusRank: Record<ConversationMessage['status'], number> = {
        sending: 0,
        streaming: 1,
        completed: 2,
        failed: 2,
        interrupted: 2,
      }
      const content = message.content.length > stored.content.length ? message.content : stored.content
      const status = statusRank[message.status] > statusRank[stored.status] ? message.status : stored.status
      persistedMessages.set(message.id, {
        ...(message.updatedAt > stored.updatedAt ? message : stored),
        content,
        status,
        ordinal: Math.min(message.ordinal, stored.ordinal),
        createdAt: Math.min(message.createdAt, stored.createdAt),
        updatedAt: Math.max(message.updatedAt, stored.updatedAt),
      })
      continue
    }
    if (!hasPersistedEquivalent(message)) persistedMessages.set(message.id, message)
  }

  return [...persistedMessages.values()].sort((left, right) => left.ordinal - right.ordinal || left.createdAt - right.createdAt)
}

function mergeApprovals(persisted: ConversationDetail['approvals'], current: ConversationDetail['approvals']) {
  const records = new Map(current.map((approval) => [approval.id, approval]))
  for (const approval of persisted) {
    const existing = records.get(approval.id)
    if (existing && existing.status !== 'pending' && approval.status === 'pending') continue
    records.set(approval.id, approval)
  }
  return [...records.values()]
}

function preferRun(persistedDetail: ConversationDetail, currentDetail: ConversationDetail) {
  const persisted = persistedDetail.lastRun
  const current = currentDetail.lastRun
  if (!persisted) return current
  if (!current) return persisted
  if (current.id.startsWith('pending-run-')) return persisted
  if (persisted.id !== current.id) return conversationRunIsActive(currentDetail) ? current : persisted
  if (persisted.lastSeq > current.lastSeq) return persisted
  if (persisted.lastSeq < current.lastSeq) return current
  return runRecordIsActive(current) && !runRecordIsActive(persisted) ? persisted : current
}

export function mergeConversationDetail(persisted: ConversationDetail, current: ConversationDetail | null) {
  if (!current || current.conversation.id !== persisted.conversation.id) return persisted
  const lastRun = preferRun(persisted, current)
  const kernelSnapshot = mergeKernelSnapshot(persisted, current, lastRun?.id)
  const persistedSnapshot = snapshotForRun(persisted)
  const currentSnapshot = snapshotForRun(current)
  const staleRunRead = Boolean(persisted.lastRun?.id !== lastRun?.id
    || persisted.lastRun?.id === current.lastRun?.id && (persisted.lastRun?.lastSeq ?? 0) < (current.lastRun?.lastSeq ?? 0)
    || persistedSnapshot?.runId === currentSnapshot?.runId && persistedSnapshot && currentSnapshot
      && BigInt(persistedSnapshot.lastEventSeq) < BigInt(currentSnapshot.lastEventSeq))
  const deliverySource = staleRunRead ? current : persisted
  // A checklist belongs to one Run. Do not borrow older task verdicts when a
  // slow conversation read loses to the currently observed execution revision.
  const deliveryChecklist = deliverySource.deliveryChecklist?.filter(item => item.runId === lastRun?.id)
  const runtimeEvents = mergeRecords(
    persisted.runtimeEvents,
    current.runtimeEvents,
    (event) => `${event.runId}:${event.seq}`,
  ).sort((left, right) => left.createdAt - right.createdAt || left.seq - right.seq)

  return {
    ...persisted,
    kernelSnapshot,
    deliveryChecklist,
    deliveryChecklistTruncated: deliverySource.deliveryChecklistTruncated,
    messages: mergeMessages(persisted.messages, current.messages.filter(message => {
      if (!previewBelongsToSnapshot(message, kernelSnapshot)) return false
      // A complete run window omitting a settled Kernel row means the Host
      // superseded it. Keep unrelated, previously paged history untouched.
      return !message.id.startsWith('kernel-message:') || !!message.kernelPreview
        || persisted.messages.some(item => item.id === message.id)
        || !persisted.messages.some(item => item.role === 'user' && item.runId === message.runId)
    })),
    runtimeEvents,
    toolCalls: mergeRecords(persisted.toolCalls, current.toolCalls, (toolCall) => toolCall.id),
    approvals: mergeApprovals(persisted.approvals, current.approvals),
    attachments: mergeRecords(persisted.attachments, current.attachments, (attachment) => attachment.id),
    artifacts: mergeRecords(persisted.artifacts, current.artifacts, (artifact) => artifact.id),
    knowledgeBindings: mergeRecords(
      persisted.knowledgeBindings,
      current.knowledgeBindings,
      (binding) => `${binding.serviceConnectionId}:${binding.knowledgeBaseId}`,
    ),
    hasEarlierMessages: persisted.hasEarlierMessages || current.hasEarlierMessages,
    lastRun,
    runs: mergeRecords(persisted.runs ?? [], current.runs ?? [], (run) => run.id),
    // A0 Work Loop merge
    goals: mergeRecords(persisted.goals, current.goals, (goal) => goal.id),
    tasks: mergeRecords(persisted.tasks, current.tasks, (task) => task.id),
    evidence: mergeRecords(persisted.evidence, current.evidence, (evidence) => evidence.id),
    planRevisions: mergeRecords(persisted.planRevisions ?? [], current.planRevisions ?? [], (item) => item.id),
    reviewFindings: mergeRecords(persisted.reviewFindings ?? [], current.reviewFindings ?? [], (item) => item.id),
    acceptances: mergeRecords(persisted.acceptances ?? [], current.acceptances ?? [], (item) => item.id),
    childRuns: mergeRecords(persisted.childRuns ?? [], current.childRuns ?? [], (item) => item.childRunId),
  }
}

const processEventTypes = new Set([
  'run.started',
  'run.request_snapshot',
  'run.phase',
  'run.model_waiting',
  'run.retrying',
  'run.retry.completed',
  'context.compaction.started',
  'context.compaction.dispatched',
  'context.compaction.completed',
  'context.compaction.failed',
  'planner.started',
  'planner.completed',
  'planner.failed',
  'message.started',
  'message.delta',
  'reasoning.delta',
  'tool.started',
  'tool.updated',
  'tool.completed',
  'user.question.requested',
  'user.question.responded',
  'source.added',
  'usage.updated',
  'message.completed',
  'run.completed',
  'run.cancelled',
  'run.failed',
  'run.interrupted',
  // A0 Work Loop events
  'goal.proposed',
  'goal.activated',
  'goal.blocked',
  'goal.completed',
  'goal.cancelled',
  'task.created',
  'task.started',
  'task.completed',
  'task.blocked',
  'task.interrupted',
  'evidence.added',
  'evidence.validated',
  'plan.revised',
  'plan.approved',
  'plan.rejected',
  'review.finding_added',
  'review.finding_resolved',
  'acceptance.completed',
])

function updateAssistantMessage(
  messages: ConversationMessage[],
  notification: RuntimeEventNotification,
  update: (message: ConversationMessage) => ConversationMessage,
) {
  const messageId = `assistant-${notification.runId}`
  const existingIndex = messages.findIndex((message) => (
    message.id === messageId || (message.role === 'assistant' && message.runId === notification.runId)
  ))
  if (existingIndex >= 0) {
    const next = [...messages]
    next[existingIndex] = update(next[existingIndex])
    return next
  }

  const timestamp = Date.parse(notification.timestamp) || Date.now()
  const ordinal = messages.reduce((highest, message) => Math.max(highest, message.ordinal), 0) + 1
  const created: ConversationMessage = {
    id: messageId,
    conversationId: notification.conversationId,
    runId: notification.runId,
    role: 'assistant',
    kind: 'text',
    content: '',
    status: 'streaming',
    ordinal,
    createdAt: timestamp,
    updatedAt: timestamp,
  }
  return [...messages, update(created)]
}

export function applyRuntimeNotification(
  current: ConversationDetail | null,
  notification: RuntimeEventNotification,
) {
  if (!current || current.conversation.id !== notification.conversationId) return current
  const event = notification.event
  const eventData = objectValue(event.data)
  const goalEvent = objectValue(eventData?.goal) ?? event
  const taskEvent = objectValue(eventData?.task) ?? event
  const evidenceEvent = objectValue(eventData?.evidence) ?? eventData ?? event
  const planRevisionEvent = objectValue(eventData?.planRevision)
  const reviewFindingEvent = objectValue(eventData?.finding)
  const acceptanceEvent = objectValue(eventData?.acceptance)
  const goalId = typeof event.goalId === 'string'
    ? event.goalId
    : typeof goalEvent.id === 'string' ? goalEvent.id : null
  const taskId = typeof event.taskId === 'string'
    ? event.taskId
    : typeof taskEvent.id === 'string' ? taskEvent.id : null
  const currentRun = current.lastRun
  // This stream is Legacy Engine output, not a Kernel commit notification.
  // Once authority is known, only persisted snapshots may advance this run.
  if (snapshotForRun(current)?.runId === notification.runId) return current
  const sameRun = currentRun?.id === notification.runId
  const replacesPendingRun = Boolean(currentRun?.id.startsWith('pending-run-'))
  const updatesActiveRun = !currentRun || sameRun || replacesPendingRun
  const alreadyProcessed = processEventTypes.has(event.type)
    && current.runtimeEvents.some((item) => (
      item.runId === notification.runId && item.seq === notification.seq
    ))
  if (alreadyProcessed) return current
  if (sameRun && currentRun && notification.seq <= currentRun.lastSeq) return current

  const timestamp = Date.parse(notification.timestamp) || Date.now()
  const workTimestamp = notification.timestamp
  let messages = current.messages
  if (updatesActiveRun && event.type === 'message.started') {
    messages = updateAssistantMessage(messages, notification, (message) => ({ ...message, status: 'streaming', updatedAt: timestamp }))
  } else if (updatesActiveRun && event.type === 'message.delta' && typeof event.delta === 'string') {
    messages = updateAssistantMessage(messages, notification, (message) => ({
      ...message,
      content: message.content + event.delta,
      status: 'streaming',
      updatedAt: timestamp,
    }))
  } else if (updatesActiveRun && (event.type === 'message.completed' || event.type === 'run.completed')) {
    messages = updateAssistantMessage(messages, notification, (message) => ({ ...message, status: 'completed', updatedAt: timestamp }))
  } else if (updatesActiveRun && (event.type === 'run.cancelled' || event.type === 'run.failed' || event.type === 'run.interrupted')) {
    messages = updateAssistantMessage(messages, notification, (message) => ({ ...message, status: 'interrupted', updatedAt: timestamp }))
  }

  const runtimeEvents = processEventTypes.has(event.type)
    && !current.runtimeEvents.some((item) => item.runId === notification.runId && item.seq === notification.seq)
    ? [...current.runtimeEvents, {
        runId: notification.runId,
        seq: notification.seq,
        eventType: event.type,
        event: event.type === 'message.delta'
          ? { type: 'message.delta', deltaLength: typeof event.delta === 'string' ? event.delta.length : -1,
              deltaFingerprint: typeof event.delta === 'string' ? answerDeltaFingerprint(event.delta) : null }
          : event,
        createdAt: timestamp,
      }]
    : current.runtimeEvents

  // A0 Work Loop optimistic updates
  let goals = current.goals
  let tasks = current.tasks
  let evidence = current.evidence
  let planRevisions = current.planRevisions ?? []
  let reviewFindings = current.reviewFindings ?? []
  let acceptances = current.acceptances ?? []

  const goalStatus = event.type === 'goal.proposed'
    ? 'proposed'
    : event.type === 'goal.activated'
      ? 'active'
      : event.type === 'goal.blocked'
        ? 'blocked'
        : event.type === 'goal.completed' || event.type === 'acceptance.completed'
          ? 'completed'
          : event.type === 'goal.cancelled'
            ? 'cancelled'
            : null

  const hasRenderableGoalPayload = typeof goalEvent.title === 'string'
    && goalEvent.title.trim().length > 0
    && typeof goalEvent.objective === 'string'
    && goalEvent.objective.trim().length > 0

  if (goalStatus && goalId && !goals.some((goal) => goal.id === goalId) && hasRenderableGoalPayload) {
      goals = [...goals, {
        id: goalId,
        conversationId: notification.conversationId,
        title: typeof goalEvent.title === 'string' ? goalEvent.title : '',
        objective: typeof goalEvent.objective === 'string' ? goalEvent.objective : '',
        acceptanceSummary: typeof goalEvent.acceptanceSummary === 'string'
          ? goalEvent.acceptanceSummary
          : null,
        status: goalStatus as GoalRecord['status'],
        version: typeof goalEvent.version === 'number' ? goalEvent.version : 1,
        createdBy: typeof goalEvent.createdBy === 'string'
          ? goalEvent.createdBy
          : notification.runId,
        createdAt: typeof goalEvent.createdAt === 'string'
          ? goalEvent.createdAt
          : workTimestamp,
        updatedAt: typeof goalEvent.updatedAt === 'string'
          ? goalEvent.updatedAt
          : workTimestamp,
        completedAt: goalStatus === 'completed'
          ? (typeof goalEvent.completedAt === 'string' ? goalEvent.completedAt : workTimestamp)
          : null,
        blockedReason: typeof goalEvent.blockedReason === 'string'
          ? goalEvent.blockedReason
          : null,
      }]
  } else if (goalStatus && goalId && goals.some((goal) => goal.id === goalId)) {
    goals = goals.map((goal) => {
      if (goal.id !== goalId) return goal
      return {
        ...goal,
        ...(hasRenderableGoalPayload ? {
          title: goalEvent.title as string,
          objective: goalEvent.objective as string,
          acceptanceSummary: typeof goalEvent.acceptanceSummary === 'string'
            ? goalEvent.acceptanceSummary
            : null,
        } : {}),
        status: goalStatus as GoalRecord['status'],
        version: typeof goalEvent.version === 'number' ? goalEvent.version : goal.version,
        updatedAt: typeof goalEvent.updatedAt === 'string'
          ? goalEvent.updatedAt
          : workTimestamp,
        completedAt: goalStatus === 'completed'
          ? (typeof goalEvent.completedAt === 'string' ? goalEvent.completedAt : workTimestamp)
          : goal.completedAt,
        blockedReason: event.type === 'goal.blocked' && typeof goalEvent.blockedReason === 'string'
          ? goalEvent.blockedReason
          : goal.blockedReason,
      }
    })
  } else if (event.type === 'task.created' && taskId && goalId) {
    const exists = tasks.some((task) => task.id === taskId)
    if (!exists) {
      tasks = [...tasks, {
        id: taskId,
        goalId,
        parentTaskId: typeof taskEvent.parentTaskId === 'string' ? taskEvent.parentTaskId : null,
        ordinal: typeof taskEvent.ordinal === 'number' ? taskEvent.ordinal : 0,
        title: typeof taskEvent.title === 'string' ? taskEvent.title : '',
        detail: typeof taskEvent.detail === 'string' ? taskEvent.detail : null,
        status: 'queued' as const,
        ownerRunId: null,
        attempt: typeof taskEvent.attempt === 'number' ? taskEvent.attempt : 1,
        version: typeof taskEvent.version === 'number' ? taskEvent.version : 1,
        blockedReason: null,
        createdAt: typeof taskEvent.createdAt === 'string'
          ? taskEvent.createdAt
          : workTimestamp,
        updatedAt: typeof taskEvent.updatedAt === 'string'
          ? taskEvent.updatedAt
          : workTimestamp,
        startedAt: null,
        finishedAt: null,
      }]
    }
  } else if (event.type === 'task.started' && taskId) {
    tasks = tasks.map((task) => {
      if (task.id !== taskId) return task
      return {
        ...task,
        status: 'in_progress' as const,
        ownerRunId: typeof taskEvent.ownerRunId === 'string'
          ? taskEvent.ownerRunId
          : task.ownerRunId,
        attempt: typeof taskEvent.attempt === 'number' ? taskEvent.attempt : task.attempt,
        version: typeof taskEvent.version === 'number' ? taskEvent.version : task.version,
        updatedAt: typeof taskEvent.updatedAt === 'string'
          ? taskEvent.updatedAt
          : workTimestamp,
        startedAt: task.startedAt
          ?? (typeof taskEvent.startedAt === 'string' ? taskEvent.startedAt : workTimestamp),
      }
    })
  } else if ((event.type === 'task.completed' || event.type === 'task.blocked' || event.type === 'task.interrupted') && taskId) {
    tasks = tasks.map((task) => {
      if (task.id !== taskId) return task
      const status = event.type === 'task.completed' ? 'completed' : event.type === 'task.blocked' ? 'blocked' : 'interrupted'
      return {
        ...task,
        status: status as WorkTaskRecord['status'],
        version: typeof taskEvent.version === 'number' ? taskEvent.version : task.version,
        updatedAt: typeof taskEvent.updatedAt === 'string'
          ? taskEvent.updatedAt
          : workTimestamp,
        finishedAt: status === 'completed'
          ? (typeof taskEvent.finishedAt === 'string' ? taskEvent.finishedAt : workTimestamp)
          : task.finishedAt,
        blockedReason: event.type === 'task.blocked' && typeof taskEvent.blockedReason === 'string'
          ? taskEvent.blockedReason
          : task.blockedReason,
      }
    })
  } else if (event.type === 'evidence.added' && typeof evidenceEvent.id === 'string' && taskId) {
    const exists = evidence.some((item) => item.id === evidenceEvent.id)
    if (!exists) {
      evidence = [...evidence, {
        id: evidenceEvent.id,
        taskId,
        sourceRunId: typeof evidenceEvent.sourceRunId === 'string'
          ? evidenceEvent.sourceRunId
          : notification.runId,
        evidenceType: typeof evidenceEvent.evidenceType === 'string'
          ? evidenceEvent.evidenceType as TaskEvidenceRecord['evidenceType']
          : 'tool_call',
        refKind: typeof evidenceEvent.refKind === 'string'
          ? evidenceEvent.refKind as TaskEvidenceRecord['refKind']
          : 'tool_call',
        refId: typeof evidenceEvent.refId === 'string' ? evidenceEvent.refId : '',
        summary: typeof evidenceEvent.summary === 'string' ? evidenceEvent.summary : '',
        metadata: objectValue(evidenceEvent.metadata) ?? {},
        validityStatus: typeof evidenceEvent.validityStatus === 'string'
          ? evidenceEvent.validityStatus as TaskEvidenceRecord['validityStatus']
          : 'unverified',
        traceId: typeof evidenceEvent.traceId === 'string' ? evidenceEvent.traceId : null,
        spanId: typeof evidenceEvent.spanId === 'string' ? evidenceEvent.spanId : null,
        checkedAt: typeof evidenceEvent.checkedAt === 'string' ? evidenceEvent.checkedAt : null,
        invalidReason: typeof evidenceEvent.invalidReason === 'string'
          ? evidenceEvent.invalidReason
          : null,
        createdAt: typeof evidenceEvent.createdAt === 'string'
          ? evidenceEvent.createdAt
          : workTimestamp,
      }]
    }
  } else if (event.type === 'evidence.validated' && typeof evidenceEvent.evidenceId === 'string') {
    evidence = evidence.map((item) => {
      if (item.id !== evidenceEvent.evidenceId) return item
      return {
        ...item,
        validityStatus: typeof evidenceEvent.validityStatus === 'string'
          ? evidenceEvent.validityStatus as TaskEvidenceRecord['validityStatus']
          : item.validityStatus,
        checkedAt: workTimestamp,
      }
    })
  } else if (['plan.revised', 'plan.approved', 'plan.rejected'].includes(event.type) && planRevisionEvent && typeof planRevisionEvent.id === 'string') {
    planRevisions = [...planRevisions.filter((item) => item.id !== planRevisionEvent.id), planRevisionEvent as unknown as PlanRevisionRecord]
  } else if ((event.type === 'review.finding_added' || event.type === 'review.finding_resolved') && reviewFindingEvent && typeof reviewFindingEvent.id === 'string') {
    reviewFindings = [...reviewFindings.filter((item) => item.id !== reviewFindingEvent.id), reviewFindingEvent as unknown as ReviewFindingRecord]
  } else if (event.type === 'acceptance.completed' && acceptanceEvent && typeof acceptanceEvent.id === 'string') {
    acceptances = [acceptanceEvent as unknown as AcceptanceRecord, ...acceptances.filter((item) => item.id !== acceptanceEvent.id)]
  }

  return {
    ...current,
    messages,
    runtimeEvents,
    lastRun: updatesActiveRun ? eventRun(current.lastRun, notification) : current.lastRun,
    goals,
    tasks,
    evidence,
    planRevisions,
    reviewFindings,
    acceptances,
  }
}

export function applyWorkEvent(
  current: ConversationDetail | null,
  workEvent: WorkEventRecord,
) {
  if (!current || current.conversation.id !== workEvent.conversationId) return current
  const reduced = applyRuntimeNotification(
    { ...current, lastRun: null, messages: current.messages, runtimeEvents: [] },
    {
      conversationId: workEvent.conversationId,
      runtimeSessionId: null,
      runId: workEvent.runId ?? `work:${workEvent.conversationId}`,
      seq: workEvent.sequence,
      timestamp: workEvent.timestamp,
      event: {
        type: workEvent.type,
        goalId: workEvent.goalId,
        taskId: workEvent.taskId,
        traceId: workEvent.traceId,
        spanId: workEvent.spanId,
        data: workEvent.data,
      },
    },
  )
  return reduced ? {
    ...reduced,
    messages: current.messages,
    runtimeEvents: current.runtimeEvents,
    lastRun: current.lastRun,
  } : current
}

export function reduceRuntimeNotifications(
  detail: ConversationDetail,
  notifications: RuntimeEventNotification[],
) {
  const ordered = [...notifications].sort((left, right) => {
    if (left.runId === right.runId) return left.seq - right.seq
    const timestampDifference = Date.parse(left.timestamp) - Date.parse(right.timestamp)
    return timestampDifference || left.runId.localeCompare(right.runId) || left.seq - right.seq
  })
  return ordered.reduce<ConversationDetail | null>(applyRuntimeNotification, detail) ?? detail
}
