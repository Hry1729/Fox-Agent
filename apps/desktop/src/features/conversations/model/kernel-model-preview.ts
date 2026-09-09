import { snapshotForRun } from './kernel-snapshot'
import type { ConversationDetail, ConversationMessage, KernelModelPreview, KernelRunSnapshot } from './types'

export function previewBelongsToSnapshot(message: ConversationMessage, snapshot: KernelRunSnapshot | null) {
  return !message.kernelPreview || !!snapshot && snapshot.state === 'running' && snapshot.runId === message.runId
    && BigInt(snapshot.lastEventSeq) === BigInt(message.kernelPreview.checkpointSeq) + 1n
}

/** A preview changes display only: never Run facts, sequence, approvals or history on disk. */
export function applyKernelModelPreview(detail: ConversationDetail | null, value: unknown, now = Date.now()): ConversationDetail | null {
  if (!detail || !value || typeof value !== 'object' || Array.isArray(value)) return detail
  const notice = value as KernelModelPreview
  if (notice.schemaVersion !== 1 || ![notice.runId, notice.conversationId, notice.turnId].every(id => typeof id === 'string' && id.trim() && id.length <= 512)
    || !Number.isSafeInteger(notice.checkpointSeq) || notice.checkpointSeq <= 0 || !Number.isSafeInteger(notice.revision) || notice.revision <= 0
    || typeof notice.text !== 'string' || new TextEncoder().encode(notice.text).length > 262_144
    || (notice.reasoning !== undefined && (typeof notice.reasoning !== 'string' || new TextEncoder().encode(notice.reasoning).length > 262_144))
    || Object.keys(notice).some(key => !['schemaVersion', 'runId', 'conversationId', 'turnId', 'checkpointSeq', 'revision', 'text', 'reasoning'].includes(key))) return detail
  const snapshot = snapshotForRun(detail)
  if (!snapshot || detail.conversation.id !== notice.conversationId || snapshot.runId !== notice.runId || snapshot.turnId !== notice.turnId
    || snapshot.state !== 'running' || BigInt(snapshot.lastEventSeq) !== BigInt(notice.checkpointSeq) + 1n) return detail
  const id = `kernel-message:${notice.runId}:${notice.checkpointSeq}`
  if (detail.messages.some(message => message.id === id && message.status !== 'streaming')) return detail
  const previous = detail.messages.find(message => message.id === id)
  if (previous?.kernelPreview && previous.kernelPreview.revision >= notice.revision) return detail
  const message: ConversationMessage = {
    id, runId: notice.runId, conversationId: notice.conversationId, role: 'assistant', kind: 'text', status: 'streaming', content: notice.text,
    ordinal: previous?.ordinal ?? Math.max(0, ...detail.messages.map(message => message.ordinal)) + 1,
    createdAt: previous?.createdAt ?? now, updatedAt: now,
    kernelPreview: { checkpointSeq: notice.checkpointSeq, revision: notice.revision },
  }
  return { ...detail, messages: [...detail.messages.filter(message => message.id !== id && (!message.kernelPreview || message.runId !== notice.runId)), message] }
}
