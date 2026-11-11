import { createContext, useContext, useMemo, type ReactNode } from 'react'
import { useAuiState } from '@assistant-ui/react'
import type { FoxThreadMessage } from '@/features/chat/thread/convert-detail'
import { useFoxThread } from '@/features/chat/thread/fox-thread-context'
import { formatClockTime, formatDateTime, formatShortDateTime } from '@/lib/time-format'
import { visibleContactReplies } from './chat-message-model'

const ContactTimeContext = createContext<ReadonlyMap<string, number>>(new Map())

export function contactTimeGroups(rows: readonly { id: string; createdAt: number }[]): Map<string, number> {
  const groups = new Map<string, number>()
  let previous: number | undefined
  for (const row of rows) {
    if (previous === undefined || new Date(previous).toDateString() !== new Date(row.createdAt).toDateString() || row.createdAt - previous >= 15 * 60_000) {
      groups.set(row.id, row.createdAt)
    }
    previous = row.createdAt
  }
  return groups
}

export function contactGroupTimeLabel(timestamp: number, now = new Date()): string {
  const date = new Date(timestamp)
  const yesterday = new Date(now)
  yesterday.setDate(yesterday.getDate() - 1)
  if (date.toDateString() === now.toDateString()) return `今天 ${formatClockTime(date)}`
  if (date.toDateString() === yesterday.toDateString()) return `昨天 ${formatClockTime(date)}`
  return date.getFullYear() === now.getFullYear() ? formatShortDateTime(date) : formatDateTime(date)
}

export function ContactTimeGroups({ children }: { children: ReactNode }) {
  const fox = useFoxThread()
  const messages = useAuiState(state => state.thread.messages)
  const groups = useMemo(() => {
    if (!fox.contactChat) return new Map<string, number>()
    const rows = messages.flatMap(message => {
      if (message.role === 'user') return [{ id: message.id, createdAt: message.createdAt.getTime() }]
      const source = message.metadata?.custom?.fox as FoxThreadMessage | undefined
      if (!source) return []
      const run = source.runId ?? source.id
      return visibleContactReplies(source, fox.eventsByRun.get(run) ?? [], fox.contactBubblesByRun?.get(run), fox.contactKernelPreviewsByRun?.get(run))
    })
    return contactTimeGroups(rows)
  }, [messages, fox.contactChat, fox.eventsByRun, fox.contactBubblesByRun, fox.contactKernelPreviewsByRun])
  return <ContactTimeContext.Provider value={groups}>{children}</ContactTimeContext.Provider>
}

export function ContactTimeDivider({ messageId }: { messageId: string }) {
  const timestamp = useContext(ContactTimeContext).get(messageId)
  if (timestamp === undefined) return null
  return <div className="fox-contact-time-divider"><time dateTime={new Date(timestamp).toISOString()}>{contactGroupTimeLabel(timestamp)}</time></div>
}
