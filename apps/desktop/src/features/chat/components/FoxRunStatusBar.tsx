import { useEffect, useMemo, useState } from 'react'
import type { ConversationDetail } from '@/features/conversations/model/types'
import { presentConversationActivity } from '../run-activity-presentation'
import { formatRunElapsed } from '../turn-process-timing'
import { BlueFoxAvatar } from './BlueFoxAvatar'
import './fox-run-status-bar.css'

export function FoxRunStatusBar({ detail, conversationId }: { detail: ConversationDetail | null; conversationId?: string | null }) {
  // A late fetch for another conversation cannot take over this status strip.
  const presentation = useMemo(() => presentConversationActivity(detail, conversationId), [detail, conversationId])
  const current = presentation
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    setNow(Date.now())
    if (!presentation.active && (presentation.endedAt === null || Date.now() - presentation.endedAt >= 2000)) return
    const timer = window.setInterval(() => {
      setNow(Date.now())
      if (!presentation.active && presentation.endedAt !== null && Date.now() - presentation.endedAt >= 2000) clearInterval(timer)
    }, 500)
    return () => clearInterval(timer)
  }, [presentation.runId, presentation.active, presentation.endedAt])
  const endedAgo = presentation.endedAt === null ? Infinity : now - presentation.endedAt
  const dispatch = detail?.runtimeEvents.findLast(event => event.runId === presentation.runId && event.eventType === 'run.phase' && event.event.phase === 'request_sent')
  const justSent = dispatch && now - dispatch.createdAt >= 0 && now - dispatch.createdAt < 1500
  const animation = presentation.outcome === 'completed' && endedAgo >= 2000 ? 'idle'
    : current.animation === 'receiving' && justSent ? 'sending' : current.animation
  const animate = !((presentation.outcome === 'failed' || presentation.outcome === 'interrupted') && endedAgo >= 1500)
  const duration = presentation.startedAt !== null && (presentation.active || presentation.endedAt !== null)
    ? formatRunElapsed((presentation.endedAt ?? now) - presentation.startedAt) : null
  return <div className="fox-run-status-bar" data-run-id={presentation.runId ?? ''}>
    <BlueFoxAvatar state={animation} progress={current.progress} animate={animate} startedAt={presentation.startedAt} />
    <span role="status" aria-live="polite" className="fox-run-status-label">{current.label}</span>
    {duration && <><span aria-hidden="true">·</span><span role="timer" className="fox-run-status-time">{duration}</span></>}
  </div>
}
