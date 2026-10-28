import { normalizeAssistantMarkdown } from '../../conversations/model/assistant-presentation'
import { conversationRunIsActive } from '../../conversations/model/kernel-snapshot'
import { deriveRunTerminalStates, type RunFailure } from '../../conversations/model/run-failure'
import type { ArtifactRecord, AttachmentRecord, ConversationDetail, ConversationMessage, RunEventRecord, RunRecord } from '../../conversations/model/types'
import { splitAssistantContent } from '../timeline/fox-assistant-content'
import { runtimeProcess, type RuntimeSource, type RuntimeToolStep } from '../timeline/fox-runtime-process-model'
import { groupAssistantContinuations, latestRunAssistantId, type RuntimeTimelineMessage } from '../runtime-timeline-performance'

/**
 * The Fox-side intermediate message model for the assistant-ui external store
 * runtime (migration step 5B). `deriveFoxThreadMessages` projects a
 * `ConversationDetail` into stable `FoxThreadMessage` objects; the runtime hook
 * feeds them through `useExternalMessageConverter` with
 * `joinStrategy: 'none'` — Fox owns its message boundaries.
 *
 * Performance contract: a message that has not changed keeps its object
 * identity across derivations, and `revision` only changes when data the
 * renderer can see changes. That lets the converter cache keep every
 * non-changing message's `ThreadMessage` object alive and re-convert only the
 * last running assistant message while it streams.
 */

/** The `runtimeProcess` grouping result carried by a `data-fox-process` part. */
export interface FoxProcessData {
  reasoning: string
  reasoningItems: string[]
  steps: Array<{ kind: 'reasoning'; id: string; seq: number; text: string } | { kind: 'tool'; seq: number; id: string }>
  tools: RuntimeToolStep[]
  sources: RuntimeSource[]
}

export interface FoxAttachment {
  id: string
  type: 'image' | 'document' | 'file'
  name: string
  contentType?: string
  status: { type: 'complete' }
  content: readonly ({ type: 'text'; text: string } | { type: 'file'; filename?: string; mimeType: string; data: string; sourceType?: 'url' | 'id' })[]
}

export type FoxThreadPart =
  | { type: 'data-fox-process'; id: string; data: FoxProcessData }
  | { type: 'text'; text: string }
  | { type: 'data-fox-artifacts'; data: ArtifactRecord[] }
  | { type: 'data-fox-error'; data: RunFailure }

export interface FoxThreadMessage {
  source?: RuntimeTimelineMessage
  id: string
  role: 'user' | 'assistant'
  /** The run this message belongs to (null for the draft-pending message). */
  runId: string | null
  createdAt: number
  status: string
  /** Whether this message's run is the currently active run. */
  isActiveRun: boolean
  /** Cached conversion key: changes exactly when visible data changes. */
  revision: string
  content: FoxThreadPart[]
  attachments: FoxAttachment[]
  /** Failure facts for the message's run, when it ended abnormally. */
  failure: RunFailure | null
  cancelled: boolean
  /** Only set on the optimistic pending user message. */
  optimistic?: boolean
}

export interface FoxThreadInput {
  detail: ConversationDetail | null
  /** The UI-level optimistic user message shown while `send` resolves. */
  pendingMessage?: ConversationMessage | null
  streamingText?: string
  enabled?: boolean
}

/** Cache entry: reuses object identity for messages whose key has not changed. */
interface FoxThreadCacheEntry {
  key: string
  message: FoxThreadMessage
}

export type FoxThreadCache = Map<string, FoxThreadCacheEntry>

export function createFoxThreadCache(): FoxThreadCache {
  return new Map()
}

function runVersion(events: readonly RunEventRecord[] | undefined, runId: string | null) {
  if (!runId || !events) return 0
  let last = 0
  for (const event of events) {
    if (event.runId === runId && event.seq > last) last = event.seq
  }
  return last
}

function attachmentMediaType(record: AttachmentRecord) {
  return record.mediaType ?? 'application/octet-stream'
}

export function attachmentToFoxAttachment(record: AttachmentRecord): FoxAttachment {
  const mediaType = attachmentMediaType(record)
  return {
    id: record.id,
    type: mediaType.startsWith('image/') ? 'image' : mediaType === 'application/pdf' ? 'document' : 'file',
    name: record.displayName,
    contentType: mediaType,
    status: { type: 'complete' },
    content: [{ type: 'file', filename: record.displayName, mimeType: mediaType, data: record.storagePath, sourceType: 'id' }],
  }
}

export function deriveFoxThreadMessages(input: FoxThreadInput, cache: FoxThreadCache = createFoxThreadCache()): FoxThreadMessage[] {
  const { detail, pendingMessage, streamingText = '' } = input
  if (!detail) {
    // A draft conversation may still show a UI-level optimistic message.
    return pendingMessage ? [optimisticMessage(pendingMessage, 'complete')] : []
  }
  const runTerminalStates = deriveRunTerminalStates({
    persistedRuns: detail.lastRun ? [...(detail.runs ?? []), detail.lastRun] : detail.runs,
    events: detail.runtimeEvents,
  })
  const activeRunId = conversationRunIsActive(detail) ? detail.lastRun?.id ?? null : null
  const attachmentsByMessage = groupAttachments(detail.attachments)
  const artifactsByRun = groupArtifacts(detail.artifacts)
  const messages: FoxThreadMessage[] = []
  const projectedMessages = groupAssistantContinuations(detail.messages.filter(message => message.role === 'user' || message.role === 'assistant'), latestRunAssistantId(detail.messages, activeRunId ?? undefined), streamingText)
  for (const message of projectedMessages) {
    if (message.role !== 'user' && message.role !== 'assistant') continue
    messages.push(toFoxThreadMessage({ message, detail, runTerminalStates, attachmentsByMessage, artifactsByRun, activeRunId, cache }))
  }
  const pending = pendingUserMessage(pendingMessage, messages)
  if (pending) messages.push(pending)
  // A run can fail before an assistant row exists. Reopening its saved detail
  // must rebuild the failure card as well as an active run's streaming bubble.
  const syntheticRunId = activeRunId ?? (detail.lastRun && runTerminalStates.failures.has(detail.lastRun.id) ? detail.lastRun.id : null)
  if (syntheticRunId && !messages.some((message) => message.role === 'assistant' && message.runId === syntheticRunId)) {
    messages.push(syntheticAssistantMessage(detail, syntheticRunId, activeRunId ? streamingText : '', runTerminalStates, artifactsByRun, cache, Boolean(activeRunId)))
  }
  return messages
}

// --- internal helpers (all pure) -------------------------------------------

interface RunTerminalStates {
  failures: Map<string, RunFailure>
  cancelled: Set<string>
}

function groupAttachments(attachments: AttachmentRecord[]) {
  const grouped = new Map<string, AttachmentRecord[]>()
  for (const attachment of attachments) {
    if (!attachment.messageId) continue
    const bucket = grouped.get(attachment.messageId)
    if (bucket) bucket.push(attachment)
    else grouped.set(attachment.messageId, [attachment])
  }
  return grouped
}

function groupArtifacts(artifacts: ArtifactRecord[]) {
  const grouped = new Map<string, ArtifactRecord[]>()
  for (const artifact of artifacts) {
    if (!artifact.runId) continue
    const bucket = grouped.get(artifact.runId)
    if (bucket) bucket.push(artifact)
    else grouped.set(artifact.runId, [artifact])
  }
  return grouped
}

function processEventsFor(events: readonly RunEventRecord[] | undefined, runId: string | null): RunEventRecord[] {
  if (!runId || !events) return []
  const collected = events.filter((event) => event.runId === runId)
  return collected.sort((left, right) => left.seq - right.seq)
}

function mergeThinkIntoProcess(process: FoxProcessData, thinkReasoning: string): FoxProcessData {
  if (!thinkReasoning) return process
  return {
    ...process,
    reasoning: [process.reasoning, thinkReasoning].filter(Boolean).join('\n\n'),
    reasoningItems: [...process.reasoningItems, ...(thinkReasoning ? [thinkReasoning] : [])],
  }
}

function hasProcessData(process: FoxProcessData) {
  return Boolean(process.reasoning || process.steps.length || process.tools.length || process.sources.length)
}

function toFoxThreadMessage(args: {
  message: RuntimeTimelineMessage
  detail: ConversationDetail
  runTerminalStates: RunTerminalStates
  attachmentsByMessage: Map<string, AttachmentRecord[]>
  artifactsByRun: Map<string, ArtifactRecord[]>
  activeRunId: string | null
  cache: FoxThreadCache
}): FoxThreadMessage {
  const { message, detail, runTerminalStates, attachmentsByMessage, artifactsByRun, activeRunId, cache } = args
  const events = detail.runtimeEvents
  const failure = message.runId ? runTerminalStates.failures.get(message.runId) ?? null : null
  const cancelled = message.runId ? runTerminalStates.cancelled.has(message.runId) : false
  const isActiveRun = Boolean(activeRunId && message.runId === activeRunId)
  // A message's key changes when: its row changed (updatedAt/status), its run's
  // event stream advanced, or its run's terminal state arrived. Static history
  // messages never change keys, so the converter keeps reusing their objects.
  const version = runVersion(events, message.runId)
  const artifactVersion = JSON.stringify(message.runId ? artifactsByRun.get(message.runId) ?? [] : [])
  const key = `${message.id}:${message.updatedAt}:${message.status}:${version}:${isActiveRun ? 1 : 0}:${failure ? `${failure.code}/${failure.stderrTail?.length ?? 0}/${failure.rpcErrorData?.length ?? 0}` : ''}:${cancelled ? 1 : 0}:${message.content}:${artifactVersion}`
  const cached = cache.get(message.id)
  if (cached && cached.key === key) return cached.message
  const attachments = (attachmentsByMessage.get(message.id) ?? []).map(attachmentToFoxAttachment)
  const parts: FoxThreadPart[] = []
  if (message.role === 'assistant') {
    const { answer, reasoning } = splitAssistantContent(message.content)
    const eventsForRun = processEventsFor(events, message.runId)
    if (message.runId && eventsForRun.length) {
      const process = mergeThinkIntoProcess(runtimeProcess(eventsForRun), reasoning)
      if (hasProcessData(process)) parts.push({ type: 'data-fox-process', id: message.runId, data: process })
    } else if (reasoning) {
      // Inline `<think>` with no runtime events still renders as a process part.
      parts.push({
        type: 'data-fox-process',
        id: `${message.id}:think`,
        data: { reasoning, reasoningItems: [reasoning], steps: [], tools: [], sources: [] },
      })
    }
    const normalized = normalizeAssistantMarkdown(answer)
    if (normalized) parts.push({ type: 'text', text: normalized })
    const runArtifacts = message.runId ? artifactsByRun.get(message.runId) : undefined
    if (runArtifacts?.length) parts.push({ type: 'data-fox-artifacts', data: runArtifacts })
    if (failure) parts.push({ type: 'data-fox-error', data: failure })
  } else if (message.content) {
    parts.push({ type: 'text', text: message.content })
  }
  const fox: FoxThreadMessage = {
    source: message,
    id: message.id,
    // The caller filters to user/assistant roles before this point.
    role: message.role as 'user' | 'assistant',
    runId: message.runId,
    createdAt: message.createdAt,
    status: message.status,
    isActiveRun,
    revision: key,
    content: parts,
    attachments,
    failure,
    cancelled,
  }
  cache.set(message.id, { key, message: fox })
  return fox
}

function optimisticMessage(message: ConversationMessage, cacheKey: string): FoxThreadMessage {
  return {
    id: message.id,
    role: 'user',
    runId: message.runId,
    createdAt: message.createdAt,
    status: message.status,
    isActiveRun: false,
    revision: `${message.id}:${cacheKey}`,
    content: [{ type: 'text', text: message.content }],
    attachments: [],
    failure: null,
    cancelled: false,
    optimistic: true,
  }
}

/** Same de-dup rule as the legacy RuntimeTimeline: the pending message is
 * dropped once a stored user message with the same content lands nearby. */
function pendingUserMessage(pendingMessage: ConversationMessage | null | undefined, messages: FoxThreadMessage[]): FoxThreadMessage | null {
  if (!pendingMessage) return null
  const alreadyStored = messages.some((message) =>
    message.role === 'user'
    && message.content.some((part) => part.type === 'text' && part.text === pendingMessage.content)
    && message.createdAt >= pendingMessage.createdAt - 3000,
  )
  if (alreadyStored) return null
  return optimisticMessage(pendingMessage, 'pending')
}

function syntheticAssistantMessage(
  detail: ConversationDetail,
  runId: string,
  streamingText: string,
  runTerminalStates: RunTerminalStates,
  artifactsByRun: Map<string, ArtifactRecord[]>,
  cache: FoxThreadCache,
  isActiveRun: boolean,
): FoxThreadMessage {
  const version = runVersion(detail.runtimeEvents, runId)
  const failure = runTerminalStates.failures.get(runId) ?? null
  const cancelled = runTerminalStates.cancelled.has(runId)
  const status = isActiveRun ? 'streaming' : failure?.kind ?? 'completed'
  const key = `assistant-${runId}:${status}:${isActiveRun}:${streamingText}:${version}:${JSON.stringify(failure)}:${cancelled}:${JSON.stringify(artifactsByRun.get(runId) ?? [])}`
  const cached = cache.get(`assistant-${runId}`)
  if (cached && cached.key === key) return cached.message
  const parts: FoxThreadPart[] = []
  const eventsForRun = processEventsFor(detail.runtimeEvents, runId)
  if (eventsForRun.length) {
    const process = runtimeProcess(eventsForRun)
    if (hasProcessData(process)) parts.push({ type: 'data-fox-process', id: runId, data: process })
  }
  const normalized = normalizeAssistantMarkdown(streamingText)
  if (normalized) parts.push({ type: 'text', text: normalized })
  const runArtifacts = artifactsByRun.get(runId)
  if (runArtifacts?.length) parts.push({ type: 'data-fox-artifacts', data: runArtifacts })
  if (failure) parts.push({ type: 'data-fox-error', data: failure })
  const fox: FoxThreadMessage = {
    source: { id: `assistant-${runId}`, timelineKey: `assistant-run-${runId}`, conversationId: detail.conversation.id, role: 'assistant', runId, kind: 'text', content: streamingText, status, ordinal: detail.messages.length + 1, createdAt: detail.lastRun?.startedAt ?? Date.now(), updatedAt: eventsForRun.at(-1)?.createdAt ?? Date.now() },
    id: `assistant-${runId}`,
    role: 'assistant',
    runId,
    createdAt: detail.lastRun?.startedAt ?? Date.now(),
    status,
    isActiveRun,
    revision: key,
    content: parts,
    attachments: [],
    failure,
    cancelled,
  }
  cache.set(`assistant-${runId}`, { key, message: fox })
  return fox
}

/** Whether a run record counts as "active" for the thread's isRunning flag.
 * Mirrors the legacy `conversationRunState` heuristic for a single run row. */
export function runRecordIsActive(run: Pick<RunRecord, 'status'> | null | undefined) {
  const status = (run?.status ?? '').trim().toLowerCase()
  return ['queued', 'running', 'waiting_approval', 'retry_scheduled', 'compacting', 'cancelling'].includes(status)
}
