import { useMemo, type ReactNode } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { summarizeRunSteps } from './run-activity-presentation'
import { formatRunElapsed, runElapsedBounds } from './turn-process-timing'
import type { RunEventRecord } from '@/features/conversations/model/types'

/** The header line is *joined* from the fields that actually have something to say,
 *  never concatenated with a separator that belongs to a field which is absent:
 *  the first visible field carries no leading "·", and two fields that meet share
 *  exactly one. */
function joinStatusFields(fields: Array<string | undefined>) {
  return fields.filter((field): field is string => Boolean(field)).join(' · ')
}

export function TurnProcessHeader({ avatar, events, active, canToggle, collapsed, status, toolStatus, onToggle }: {
  avatar?: ReactNode
  events: readonly RunEventRecord[]
  active: boolean
  canToggle: boolean
  collapsed: boolean
  status: string
  toolStatus?: string
  onToggle: () => void
}) {
  const summary = useMemo(() => summarizeRunSteps(events, active), [events, active])
  const outcome = ['过程未完成', '过程已取消', '过程已中断'].includes(status) ? status : undefined
  const elapsed = useMemo(() => {
    if (active || outcome) return undefined
    const { startedAt, endedAt } = runElapsedBounds(events)
    return startedAt !== null && endedAt !== null ? `用时 ${formatRunElapsed(endedAt - startedAt)}` : undefined
  }, [events, active, outcome])
  const toggleLine = summary ? joinStatusFields([summary, outcome, elapsed]) : joinStatusFields([status, toolStatus, elapsed])
  const toggleLabel = collapsed ? '展开过程' : '收起过程'
  const statusRow = <>
    {toggleLine && <span className="fox-process-turn-label">{toggleLine}</span>}
  </>
  return <div className="fox-process-turn-header">
    {canToggle ? <button type="button" className="fox-process-turn-toggle"
      aria-expanded={!collapsed}
      aria-label={toggleLine ? `${toggleLabel} · ${toggleLine}` : toggleLabel}
      title={toggleLabel}
      onClick={(event) => { event.currentTarget.focus(); onToggle() }}>
      {statusRow}
      {collapsed ? <ChevronRight className="fox-process-turn-disclosure-icon" size={13} aria-hidden="true" /> : <ChevronDown className="fox-process-turn-disclosure-icon" size={13} aria-hidden="true" />}
    </button> : <span className="fox-process-turn-toggle is-status" role="status">{statusRow}</span>}
  </div>
}
