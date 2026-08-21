import { useState } from 'react'
import { ChevronDown, ChevronRight, Target } from 'lucide-react'
import type { TaskEvidenceRecord } from '@/features/conversations/model/types'
import type { GoalProgressData } from '../hooks/use-goal-progress'
import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { TaskItem } from './TaskItem'
import { Button } from '@/components/ui/button'

interface GoalProgressProps {
  data: GoalProgressData
  className?: string
  defaultExpanded?: boolean
  onEvidenceClick?: (evidence: TaskEvidenceRecord) => void
  onResolveConfirmation?: (goalId: string, expectedVersion: number, approved: boolean) => Promise<boolean> | void
}

export function GoalProgress({ data, className, defaultExpanded = false, onEvidenceClick, onResolveConfirmation }: GoalProgressProps) {
  const [isExpanded, setIsExpanded] = useState(defaultExpanded)
  const [resolving, setResolving] = useState(false)
  const { goal, tasks, evidence, planRevisions = [], reviewFindings = [], acceptances = [], completedCount, totalCount, currentTask } = data

  // Group evidence by taskId for quick lookup
  const evidenceByTask = new Map<string, TaskEvidenceRecord[]>()
  evidence.forEach((ev) => {
    const list = evidenceByTask.get(ev.taskId) || []
    list.push(ev)
    evidenceByTask.set(ev.taskId, list)
  })

  return (
    <Collapsible open={isExpanded} onOpenChange={setIsExpanded} className={cn('fox-goal-progress', className)}>
      <div className="fox-goal-progress-card border rounded-lg bg-card overflow-hidden">
        {/* Header */}
        <div className="flex items-center gap-3 p-4 bg-accent/30">
          <Target className="h-5 w-5 text-blue-600 flex-shrink-0" />

          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2 mb-1">
              <h3 className="text-sm font-semibold text-foreground">
                {goal.title}
              </h3>
              <Badge
                variant={goal.status === 'active' ? 'default' : 'secondary'}
                className="text-[10px] px-1.5 py-0"
              >
                {goal.status === 'active'
                  ? '进行中'
                  : goal.status === 'blocked'
                    ? '已阻塞'
                    : goal.status === 'completed'
                      ? '已完成'
                      : goal.status === 'proposed'
                        ? '待确认'
                      : goal.status}
              </Badge>
            </div>

            <div className="flex items-center gap-3 text-xs text-muted-foreground">
              <span className="font-medium">
                {completedCount} / {totalCount} 任务已完成
              </span>
              {currentTask && (
                <span className="text-blue-600">
                  当前: {currentTask.title}
                </span>
              )}
            </div>

            {goal.blockedReason && (
              <div className="text-xs text-yellow-700 bg-yellow-100 px-2 py-1 rounded mt-2">
                <span className="font-medium">阻塞原因: </span>
                {goal.blockedReason}
              </div>
            )}
          </div>

          <CollapsibleTrigger asChild>
            <button className="flex-shrink-0 p-1.5 hover:bg-accent rounded transition-colors">
              {isExpanded ? (
                <ChevronDown className="h-4 w-4 text-muted-foreground" />
              ) : (
                <ChevronRight className="h-4 w-4 text-muted-foreground" />
              )}
            </button>
          </CollapsibleTrigger>
        </div>

        {goal.status === 'proposed' && onResolveConfirmation && (
          <div className="border-t bg-card px-4 py-3">
            <p className="mb-3 text-sm text-muted-foreground">Fox 将为这项工作跟踪任务、执行过程和证据。是否进入工作模式并继续执行？</p>
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={resolving}
                onClick={async () => {
                  setResolving(true)
                  try { await onResolveConfirmation(goal.id, goal.version, false) } finally { setResolving(false) }
                }}
              >暂不执行</Button>
              <Button
                size="sm"
                disabled={resolving}
                onClick={async () => {
                  setResolving(true)
                  try { await onResolveConfirmation(goal.id, goal.version, true) } finally { setResolving(false) }
                }}
              >继续执行</Button>
            </div>
          </div>
        )}

        {/* Task List */}
        <CollapsibleContent>
          <div className="p-4 space-y-2 max-h-[500px] overflow-y-auto">
            {goal.objective && (
              <div className="mb-3 p-3 bg-muted/50 rounded-lg">
                <div className="text-xs font-medium text-muted-foreground mb-1">目标:</div>
                <div className="text-sm text-foreground/90">{goal.objective}</div>
              </div>
            )}

            {tasks.length === 0 ? (
              <div className="text-sm text-muted-foreground text-center py-4">
                暂无任务
              </div>
            ) : (
              tasks.map((task) => (
                <TaskItem
                  key={task.id}
                  task={task}
                  evidence={evidenceByTask.get(task.id) || []}
                  onEvidenceClick={onEvidenceClick}
                />
              ))
            )}
            {(planRevisions.length > 0 || reviewFindings.length > 0 || acceptances.length > 0) && (
              <div className="fox-goal-a1-status">
                <div><b>计划修订</b><span>{planRevisions.length ? `v${Math.max(...planRevisions.map((item) => item.revision))}` : '未建立'}</span></div>
                <div><b>独立审查</b><span>{reviewFindings.length ? `${reviewFindings.filter((item) => item.status === 'open').length} 个待处理` : '未审查'}</span></div>
                <div><b>最终验收</b><span>{acceptances[0]?.status === 'accepted' ? '已通过' : acceptances[0]?.status === 'rejected' ? '未通过' : '待验收'}</span></div>
              </div>
            )}
          </div>
        </CollapsibleContent>
      </div>
    </Collapsible>
  )
}
