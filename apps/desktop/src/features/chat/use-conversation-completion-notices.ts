import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { ConversationSummary } from '@/features/conversations/model/types'
import { conversationRunActivity, type ConversationRunIndicator, type ConversationRunOverlay } from './conversation-run-indicator'

export const COMPLETION_NOTICES_KEY = 'fox.sidebar.completedRuns.seen.v1'
export const COMPLETION_HISTORY_INITIALIZED_KEY = 'fox.sidebar.completedRuns.historyInitialized.v1'
type NoticeStorage = Pick<Storage, 'getItem' | 'setItem'>

function noticeStorage(): NoticeStorage | null {
  try { return typeof window === 'undefined' ? null : window.localStorage } catch { return null }
}

function historyWasInitialized(storage: NoticeStorage | null) {
  try { return storage?.getItem(COMPLETION_HISTORY_INITIALIZED_KEY) === '1' } catch { return false }
}

export function readAcknowledgedCompletions(storage: NoticeStorage | null): ReadonlyMap<string, string> {
  try {
    const value: unknown = JSON.parse(storage?.getItem(COMPLETION_NOTICES_KEY) ?? '[]')
    if (!Array.isArray(value)) return new Map()
    return new Map(value.filter((item): item is [string, string] => Array.isArray(item)
      && item.length === 2 && item.every(value => typeof value === 'string' && value.length > 0)))
  } catch { return new Map() }
}

/** A success notice belongs to one Run, rather than permanently to its conversation. */
export function useConversationCompletionNotices(
  conversations: readonly ConversationSummary[],
  overlay: ConversationRunOverlay,
  snapshotAt: number,
  storage: NoticeStorage | null = noticeStorage(),
  initializeHistory = false,
) {
  const [acknowledged, setAcknowledged] = useState(() => readAcknowledgedCompletions(storage))
  const [historyInitialized, setHistoryInitialized] = useState(() => historyWasInitialized(storage))
  const mountedAt = useRef(Date.now())
  const activity = useMemo(() => new Map(conversations.map(conversation => [
    conversation.id, conversationRunActivity(conversation, overlay, snapshotAt),
  ])), [conversations, overlay, snapshotAt])
  useLayoutEffect(() => {
    if (!initializeHistory || historyInitialized || snapshotAt <= 0) return
    setAcknowledged(previous => {
      const next = new Map(previous)
      for (const conversation of conversations) {
        const current = activity.get(conversation.id)
        const live = overlay.get(conversation.id)
        // Results produced while the first list was loading remain unread.
        if (current?.indicator !== 'completed' || !current.runId || conversation.activeRunId
          || Math.max(conversation.updatedAt ?? 0, conversation.lastMessageAt ?? 0) >= mountedAt.current
          || (live?.runId === current.runId && live.updatedAt >= mountedAt.current)) continue
        next.set(conversation.id, current.runId)
      }
      return next
    })
    setHistoryInitialized(true)
  }, [activity, conversations, historyInitialized, initializeHistory, overlay, snapshotAt])
  const runIndicators = useMemo(() => {
    const result = new Map<string, ConversationRunIndicator>()
    for (const [id, { indicator, runId }] of activity) {
      if (indicator && !(indicator === 'completed' && runId && acknowledged.get(id) === runId)) result.set(id, indicator)
    }
    return result
  }, [activity, acknowledged])
  const acknowledgeCompletion = useCallback((conversationId: string) => {
    const current = activity.get(conversationId)
    if (current?.indicator !== 'completed' || !current.runId) return
    const runId = current.runId
    setAcknowledged(previous => {
      if (previous.get(conversationId) === runId) return previous
      const next = new Map(previous)
      next.set(conversationId, runId)
      return next
    })
  }, [activity])
  useEffect(() => {
    try {
      storage?.setItem(COMPLETION_NOTICES_KEY, JSON.stringify([...acknowledged]))
      if (historyInitialized) storage?.setItem(COMPLETION_HISTORY_INITIALIZED_KEY, '1')
    } catch { /* Keep this session's acknowledgement. */ }
  }, [storage, acknowledged, historyInitialized])
  return { runIndicators, acknowledgeCompletion }
}

export function useVisibleCompletionAcknowledgement(options: {
  visible: boolean
  conversationId: string | null
  displayedConversationId: string | null
  acknowledgeCompletion: (conversationId: string) => void
}) {
  const current = useRef(options)
  useLayoutEffect(() => { current.current = options })
  const acknowledge = useCallback(() => {
    const { visible, conversationId, displayedConversationId, acknowledgeCompletion } = current.current
    if (visible && !document.hidden && conversationId && displayedConversationId === conversationId) acknowledgeCompletion(conversationId)
  }, [])
  // A new completion in the already visible conversation remains a notice until
  // an interaction. Only actual navigation/display changes acknowledge here.
  useEffect(() => { acknowledge() }, [options.visible, options.conversationId, options.displayedConversationId, acknowledge])
  useEffect(() => {
    window.addEventListener('focus', acknowledge)
    document.addEventListener('visibilitychange', acknowledge)
    return () => {
      window.removeEventListener('focus', acknowledge)
      document.removeEventListener('visibilitychange', acknowledge)
    }
  }, [acknowledge])
  return acknowledge
}
