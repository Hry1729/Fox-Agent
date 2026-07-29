import type {
  ConversationDetail,
  ConversationMessage,
  RunRecord,
  RuntimeEventNotification,
} from './types'

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
    if (existing?.status !== 'pending' && approval.status === 'pending') continue
    records.set(approval.id, approval)
  }
  return [...records.values()]
}

function preferRun(persisted: RunRecord | null, current: RunRecord | null) {
  if (!persisted) return current
  if (!current) return persisted
  if (current.id.startsWith('pending-run-')) return persisted
  if (persisted.id !== current.id) return runRecordIsActive(current) ? current : persisted
  if (persisted.lastSeq > current.lastSeq) return persisted
  if (persisted.lastSeq < current.lastSeq) return current
  return runRecordIsActive(current) && !runRecordIsActive(persisted) ? persisted : current
}

export function mergeConversationDetail(persisted: ConversationDetail, current: ConversationDetail | null) {
  if (!current || current.conversation.id !== persisted.conversation.id) return persisted
  const runtimeEvents = mergeRecords(
    persisted.runtimeEvents,
    current.runtimeEvents,
    (event) => `${event.runId}:${event.seq}`,
  ).sort((left, right) => left.createdAt - right.createdAt || left.seq - right.seq)

  return {
    ...persisted,
    messages: mergeMessages(persisted.messages, current.messages),
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
    lastRun: preferRun(persisted.lastRun, current.lastRun),
  }
}

const processEventTypes = new Set([
  'run.started',
  'message.started',
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
  const currentRun = current.lastRun
  const sameRun = currentRun?.id === notification.runId
  const replacesPendingRun = Boolean(currentRun?.id.startsWith('pending-run-'))
  const updatesActiveRun = !currentRun || sameRun || replacesPendingRun
  if (sameRun && currentRun && notification.seq <= currentRun.lastSeq) return current

  const event = notification.event
  const timestamp = Date.parse(notification.timestamp) || Date.now()
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
        event,
        createdAt: timestamp,
      }]
    : current.runtimeEvents

  return {
    ...current,
    messages,
    runtimeEvents,
    lastRun: updatesActiveRun ? eventRun(current.lastRun, notification) : current.lastRun,
  }
}

export function reduceRuntimeNotifications(
  detail: ConversationDetail,
  notifications: RuntimeEventNotification[],
) {
  return notifications.reduce<ConversationDetail | null>(applyRuntimeNotification, detail) ?? detail
}
