import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import type { RunEventRecord } from '@/features/conversations/model/types'
import { formatRunElapsed, runElapsedBounds } from './turn-process-timing'

function TurnElapsed({ events, active }: { events: readonly RunEventRecord[]; active: boolean }) {
  const { startedAt, endedAt } = useMemo(() => runElapsedBounds(events), [events])
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (startedAt === null || endedAt !== null || !active) return
    setNow(Date.now())
    const interval = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(interval)
  }, [startedAt, endedAt, active])
  if (startedAt === null || endedAt === null && !active) return null
  const duration = (endedAt ?? now) - startedAt
  return <span className="fox-process-turn-elapsed" role="timer">{endedAt === null ? '已用时' : '用时'} {formatRunElapsed(duration)}</span>
}

export function TurnProcessHeader({ avatar, events, active, canToggle, collapsed, status, failedToolCount, onToggle }: {
  avatar: ReactNode
  events: readonly RunEventRecord[]
  active: boolean
  canToggle: boolean
  collapsed: boolean
  status: string
  failedToolCount: number
  onToggle: () => void
}) {
  const statusSuffix = status === '工作过程' ? '' : ` · ${status}`
  const failureSuffix = failedToolCount > 0 ? ` · ${failedToolCount} 项失败` : ''
  return <div className="fox-process-turn-header">
    {avatar}
    {canToggle ? <button type="button" className={`fox-process-turn-toggle${failedToolCount ? ' has-failed-tools' : ''}`}
      aria-expanded={!collapsed} onClick={(event) => { event.currentTarget.focus(); onToggle() }}>
      {collapsed ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
      <span>{collapsed ? '展开过程' : '收起过程'}{statusSuffix}{failureSuffix}</span>
    </button> : <span className={`fox-process-turn-toggle is-status${failedToolCount ? ' has-failed-tools' : ''}`} role="status">{status}{failureSuffix}</span>}
    <TurnElapsed events={events} active={active} />
  </div>
}
