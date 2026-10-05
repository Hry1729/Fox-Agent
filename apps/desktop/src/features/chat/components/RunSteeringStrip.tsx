// Mid-run supplementary requests ("运行中补充要求") for the conversation's
// active authoritative Run.
//
// This component is deliberately **not** an editor: the main composer is the one
// place text is typed, and it owns both submit lanes. Rendering a second
// textarea here produced two drafts over one durable queue whose states could
// disagree. What remains is the audit view — the four durable states of every
// request, plus the explicit entry point for text that was queued for the next
// task and is still waiting after this Run ended.
//
// The text is durably queued by the Host, spliced into model input only at the
// next dispatch boundary, grants no tools and never rewrites a frozen request.
import { ChevronDown, Clock3, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  steeringStatusCopy,
  steeringLaneOf,
  STEERING_LANE_NEXT_TURN,
  type SteeringRowView,
} from './steering-presentation'
import type { RunSteeringState } from '../hooks/use-run-steering'

function rowState(row: SteeringRowView) {
  return row.status === 'received' && steeringLaneOf(row) === STEERING_LANE_NEXT_TURN
    ? 'queued'
    : row.status
}

export function RunSteeringStrip({
  steering,
  open,
  onOpenChange,
  onQueueForNextTask,
}: {
  steering: RunSteeringState
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Carry a held `next-turn` request into the composer of the next task. */
  onQueueForNextTask?: (content: string) => void
}) {
  const { view, rows, busy, error, discard } = steering

  if (rows.length === 0) return null

  const pendingCount = view.pending + view.queued.length

  return (
    <div className="fox-steering-strip">
      <button
        type="button"
        className="fox-steering-header"
        aria-expanded={open}
        onClick={() => onOpenChange(!open)}
      >
        <Clock3 size={13} />
        <span>{view.label}</span>
        {pendingCount > 0 && <span className="fox-steering-count">{pendingCount}</span>}
        {view.queued.length > 0 && (
          <span className="fox-steering-lane-badge">排队 {view.queued.length}</span>
        )}
        <ChevronDown size={13} className={`fox-steering-chevron ${open ? 'is-open' : ''}`} />
      </button>
      {open && (
        <div className="fox-steering-body">
          <p className="fox-steering-hint">{view.hint}</p>
          <ul className="fox-steering-list">
            {rows.map((row) => {
              const state = rowState(row)
              const copy = steeringStatusCopy(row.status, row.lane)
              const lane = steeringLaneOf(row)
              return (
                <li key={row.messageId} className={`fox-steering-item is-${state}`}>
                  <span
                    className={`fox-steering-lane is-${lane}`}
                    title={lane === STEERING_LANE_NEXT_TURN ? '排队，下一轮处理' : '补充到当前任务'}
                  >
                    {lane === STEERING_LANE_NEXT_TURN ? '下一轮' : '当前任务'}
                  </span>
                  <span className="fox-steering-text">{row.content}</span>
                  <span className="fox-steering-status" title={copy.title}>{copy.label}</span>
                  {lane === STEERING_LANE_NEXT_TURN && row.status === 'received' && discard && (
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      className="fox-steering-discard"
                      disabled={busy}
                      title="从队列中移除这条补充要求"
                      onClick={() => void discard(row.messageId)}
                    >
                      <Trash2 size={12} />
                      移除
                    </Button>
                  )}
                </li>
              )
            })}
          </ul>
          {view.retainedForNextTask && view.queued.length > 0 && (
            <div className="fox-steering-retained">
              <p>
                以下内容在本任务中没有被应用，也没有被丢弃：它们排在「下一轮」通道，只会随你下一次发起任务时处理。
              </p>
              <ul className="fox-steering-list">
                {view.queued.map((row) => (
                  <li key={`retained:${row.messageId}`} className="fox-steering-item is-queued">
                    <span className="fox-steering-text">{row.content}</span>
                    {onQueueForNextTask && (
                      <Button
                        type="button"
                        size="sm"
                        variant="outline"
                        className="fox-steering-carry"
                        onClick={() => onQueueForNextTask(row.content)}
                      >
                        放入输入框
                      </Button>
                    )}
                    {discard && (
                      <Button
                        type="button"
                        size="sm"
                        variant="ghost"
                        className="fox-steering-discard"
                        disabled={busy}
                        onClick={() => void discard(row.messageId)}
                      >
                        <Trash2 size={12} />
                        移除
                      </Button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {error && <p className="fox-steering-error">{error}</p>}
        </div>
      )}
    </div>
  )
}
