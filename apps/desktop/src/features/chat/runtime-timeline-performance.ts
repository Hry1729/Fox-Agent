import type { ArtifactRecord, ConversationMessage, RunEventRecord } from '@/features/conversations/model/types'

export function latestRunAssistantId(messages: readonly ConversationMessage[], runId?: string) {
  if (!runId) return undefined
  for (let index = messages.length - 1; index >= 0; index--) {
    const message = messages[index]
    if (message.role === 'assistant' && message.runId === runId) return message.id
  }
  return undefined
}

export function assistantDisplayContent(message: ConversationMessage, streamTargetId: string | undefined, streamingText: string) {
  return message.id === streamTargetId ? streamingText || message.content : message.content
}

export type RuntimeTimelineMessage = ConversationMessage & { timelineKey: string }

/** A model continuation is part of its existing run, not another user-facing turn.
 * Keep the first position and a stable React key, but the latest real message ID
 * for feedback/fork actions. Never modify persisted messages or merge across users.
 */
export function groupAssistantContinuations(
  messages: readonly ConversationMessage[],
  streamTargetId?: string,
  streamingText = '',
): RuntimeTimelineMessage[] {
  const result: RuntimeTimelineMessage[] = []
  let userId = 'history'
  let parts: string[] = []
  const finishGroup = () => {
    const last = result.at(-1)
    if (last?.role === 'assistant') last.content = parts.join('\n\n')
    parts = []
  }
  for (const message of messages) {
    const previous = result.at(-1)
    if (message.role !== 'assistant') {
      finishGroup()
      if (message.role === 'user') userId = message.id
      result.push({ ...message, timelineKey: message.id })
      continue
    }
    const content = assistantDisplayContent(message, streamTargetId, streamingText)
    const sameRun = previous?.role === 'assistant' && Boolean(message.runId) && previous.runId === message.runId
    if (!sameRun) finishGroup()
    if (content.trim()) parts.push(content)
    const grouped = {
      ...message,
      content: '',
      createdAt: sameRun ? previous.createdAt : message.createdAt,
      ordinal: sameRun ? previous.ordinal : message.ordinal,
      timelineKey: sameRun ? previous.timelineKey : message.runId ? `assistant-run-${message.runId}-${userId}` : `assistant-${message.id}`,
    }
    if (sameRun) result[result.length - 1] = grouped
    else result.push(grouped)
  }
  finishGroup()
  return result
}

export const EMPTY_RUNTIME_EVENTS: RunEventRecord[] = []
export const EMPTY_RUNTIME_ARTIFACTS: ArtifactRecord[] = []

function sameReferenceArray<T>(previous: T[] | undefined, next: T[]) {
  return Boolean(previous && previous.length === next.length && next.every((item, index) => previous[index] === item))
}

/**
 * Groups records while retaining the previous bucket when its item references
 * did not change. This keeps completed message rows memoizable during a stream.
 */
export function groupRuntimeRecords<T extends { runId?: string | null }>(records: readonly T[], previous: Map<string, T[]>) {
  const grouped = new Map<string, T[]>()
  for (const record of records) {
    if (!record.runId) continue
    const bucket = grouped.get(record.runId)
    if (bucket) bucket.push(record)
    else grouped.set(record.runId, [record])
  }
  const stable = new Map<string, T[]>()
  for (const [runId, bucket] of grouped) {
    const previousBucket = previous.get(runId)
    stable.set(runId, sameReferenceArray(previousBucket, bucket) ? previousBucket! : bucket)
  }
  return stable
}
