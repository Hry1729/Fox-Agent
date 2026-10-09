import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import type { RunEventRecord } from '@/features/conversations/model/types'
import { formatRunElapsed, runElapsedBounds } from './turn-process-timing'

/** The run's own lifecycle decides whether there is a whole-round time at all: a
 *  start plus either an end or a run that is still live. The header needs this
 *  answer too, because a separator between the status and a timer that never
 *  renders would be a dangling one. */
function hasElapsedTime(startedAt: number | null, endedAt: number | null, active: boolean): startedAt is number {
  return startedAt !== null && !(endedAt === null && !active)
}

function TurnElapsed({ startedAt, endedAt, active }: { startedAt: number | null; endedAt: number | null; active: boolean }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (startedAt === null || endedAt !== null || !active) return
    setNow(Date.now())
    const interval = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(interval)
  }, [startedAt, endedAt, active])
  if (!hasElapsedTime(startedAt, endedAt, active)) return null
  const duration = (endedAt ?? now) - startedAt
  return <span className="fox-process-turn-elapsed" role="timer">{endedAt === null ? '已用时' : '用时'} {formatRunElapsed(duration)}</span>
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
  // 工作过程 is the settled turn's default word, not a state to report, so it
  // contributes no field to the toggle's line. Everything else — 正在处理, 正在等待回复,
  // 过程未完成, 过程已取消, 过程已中断 — is that line's first field.
  const reportedStatus = status === '工作过程' ? undefined : status
  const toggleLine = joinStatusFields([reportedStatus, toolStatus])
  // Without a fold control the word is the label of the whole line, so it stays
  // even when it is the default one; only the joining changes.
  const plainLine = joinStatusFields([status, toolStatus])
  const statusLine = canToggle ? toggleLine : plainLine
  const { startedAt, endedAt } = useMemo(() => runElapsedBounds(events), [events])
  const elapsed = hasElapsedTime(startedAt, endedAt, active)
  const toggleLabel = collapsed ? '展开过程' : '收起过程'
  return <div className="fox-process-turn-header">
    {avatar}
    {canToggle ? <button type="button" className="fox-process-turn-toggle"
      aria-expanded={!collapsed}
      aria-label={toggleLine ? `${toggleLabel} · ${toggleLine}` : toggleLabel}
      title={toggleLabel}
      onClick={(event) => { event.currentTarget.focus(); onToggle() }}>
      {/* The visible row keeps only the chevron and the run's own status: the
          words 展开/收起过程 are for assistive technology and the tooltip, so the
          header reads as state rather than as a verb, and the avatar stays the
          identity rather than a second expand control. */}
      {collapsed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
      {toggleLine ? <span>{toggleLine}</span> : null}
    </button> : <span className="fox-process-turn-toggle is-status" role="status">{plainLine}</span>}
    {/* The separator belongs to the join, not to the timer: it exists exactly when
        there is both a status line to read and a whole-round time to separate it
        from, so a turn never opens on a dangling "·" and keeps no leftover gap.
        The header's own 7px gap supplies the spacing on both sides, exactly as the
        timer's old `::before` did. */}
    {statusLine && elapsed ? <span className="fox-process-turn-separator" aria-hidden="true">·</span> : null}
    <TurnElapsed startedAt={startedAt} endedAt={endedAt} active={active} />
  </div>
}
