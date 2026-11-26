import { useEffect, useMemo, useState } from 'react'
import { AlertTriangle, Check, CircleDashed, LoaderCircle, Minus, PauseCircle } from 'lucide-react'
import type { ConversationDetail } from '@/features/conversations/model/types'
import { presentConversationActivity, presentConversationPlan, type LivePlanStep } from '../run-activity-presentation'
import { formatRunElapsed } from '../turn-process-timing'
import { BlueFoxAvatar } from './BlueFoxAvatar'
import './fox-run-status-bar.css'

const planStepStyles = {
  queued: { icon: CircleDashed, label: '待开始' },
  in_progress: { icon: LoaderCircle, label: '进行中' },
  completed: { icon: Check, label: '已完成' },
  blocked: { icon: AlertTriangle, label: '受阻' },
  interrupted: { icon: PauseCircle, label: '已中断' },
  skipped: { icon: Minus, label: '已跳过' },
} satisfies Record<LivePlanStep['status'], { icon: typeof Check; label: string }>

export function FoxRunStatusBar({ detail, conversationId }: { detail: ConversationDetail | null; conversationId?: string | null }) {
  // A late fetch for another conversation cannot take over this status strip.
  const presentation = useMemo(() => presentConversationActivity(detail, conversationId), [detail, conversationId])
  const steps = useMemo(() => presentConversationPlan(detail, presentation), [detail, presentation])
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
  const duration = presentation.active && presentation.startedAt !== null
    ? formatRunElapsed(now - presentation.startedAt) : null
  return <div className="fox-run-activity-stack">
    {steps.length > 0 && <ol className="fox-live-plan" aria-label="执行计划" role="list">
      {steps.map(({ id, title, status }) => {
        const { icon: Icon, label } = planStepStyles[status]
        return <li key={id} className="fox-live-plan-step" data-status={status}>
          <span className="fox-live-plan-marker" aria-hidden="true"><Icon /></span>
          <span><span className="sr-only">{label}： </span>{title}</span>
        </li>
      })}
    </ol>}
    <div className="fox-run-status-bar" data-run-id={presentation.runId ?? ''}>
      <BlueFoxAvatar state={animation} progress={current.progress} animate={animate} startedAt={presentation.startedAt} />
      <span role="status" aria-live="polite" className="fox-run-status-label">{current.label}</span>
      {duration && <><span aria-hidden="true">·</span><span role="timer" className="fox-run-status-time">{duration}</span></>}
    </div>
  </div>
}
