import { useEffect, useId, useMemo, useState, type ReactNode } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { ThinkingIndicator } from '@/components/assistant-ui/elements/thinking-indicator'
import type { RunEventRecord } from '@/features/conversations/model/types'
import { formatRunElapsed, runElapsedBounds } from './turn-process-timing'

/** The run's own lifecycle decides whether there is a whole-round time at all: a
 *  start plus either an end or a run that is still live. The header needs this
 *  answer too, because a separator between the status and a timer that never
 *  renders would be a dangling one. */
function hasElapsedTime(startedAt: number | null, endedAt: number | null, active: boolean): startedAt is number {
  return startedAt !== null && !(endedAt === null && !active)
}

function TurnElapsed({ id, startedAt, endedAt, active }: { id?: string; startedAt: number | null; endedAt: number | null; active: boolean }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (startedAt === null || endedAt !== null || !active) return
    setNow(Date.now())
    const interval = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(interval)
  }, [startedAt, endedAt, active])
  if (!hasElapsedTime(startedAt, endedAt, active)) return null
  const duration = (endedAt ?? now) - startedAt
  return <span id={id} className="fox-process-turn-elapsed" role="timer">{endedAt === null ? '已用时' : '用时'} {formatRunElapsed(duration)}</span>
}

/** The header line is *joined* from the fields that actually have something to say,
 *  never concatenated with a separator that belongs to a field which is absent:
 *  the first visible field carries no leading "·", and two fields that meet share
 *  exactly one. */
function joinStatusFields(fields: Array<string | undefined>) {
  return fields.filter((field): field is string => Boolean(field)).join(' · ')
}

export function TurnProcessHeader({ avatar, events, active, canToggle, collapsed, status, toolStatus, onToggle }: {
  avatar: ReactNode
  events: readonly RunEventRecord[]
  active: boolean
  canToggle: boolean
  collapsed: boolean
  status: string
  toolStatus?: string
  onToggle: () => void
}) {
  const toggleLine = joinStatusFields([status, toolStatus])
  const { startedAt, endedAt } = useMemo(() => runElapsedBounds(events), [events])
  const elapsed = hasElapsedTime(startedAt, endedAt, active)
  const toggleLabel = collapsed ? '展开过程' : '收起过程'
  // 头像右侧的整行都是折叠控件：状态文字、分隔点、计时器一起进 trigger，点到任何
  // 位置都能开合。计时器挪进按钮后不再作为行内容被朗读，所以用 aria-describedby
  // 把它挂回触发器，聚焦时仍然能读到用时。
  const timerId = useId()
  const timer = <TurnElapsed id={timerId} startedAt={startedAt} endedAt={endedAt} active={active} />
  // The separator belongs to the join, not to the timer: it exists exactly when there
  // is both a status line to read and a whole-round time to separate it from, so a turn
  // never opens on a dangling "·". Inside the trigger the row's own 5px gap spaces it.
  const separator = toggleLine && elapsed
    ? <span className="fox-process-turn-separator" aria-hidden="true">·</span>
    : null
  const statusRow = <>
    {toggleLine && <ThinkingIndicator className="fox-process-turn-label" label={toggleLine} active={active} showDot />}
    {separator}
    {timer}
  </>
  return <div className="fox-process-turn-header">
    {avatar}
    {canToggle ? <button type="button" className="fox-process-turn-toggle"
      aria-expanded={!collapsed}
      aria-label={toggleLine ? `${toggleLabel} · ${toggleLine}` : toggleLabel}
      aria-describedby={elapsed ? timerId : undefined}
      title={toggleLabel}
      onClick={(event) => { event.currentTarget.focus(); onToggle() }}>
      {collapsed ? <ChevronRight className="fox-process-turn-disclosure-icon" size={13} aria-hidden="true" /> : <ChevronDown className="fox-process-turn-disclosure-icon" size={13} aria-hidden="true" />}
      {statusRow}
    </button> : <span className="fox-process-turn-toggle is-status" role="status">{statusRow}</span>}
  </div>
}
