import { useState } from 'react'
import { ChevronDown, ChevronRight, Target } from 'lucide-react'
import type { TaskEvidenceRecord } from '@/features/conversations/model/types'
import type { GoalProgressData } from '../hooks/use-goal-progress'
import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { TaskItem } from './TaskItem'
import { Button } from '@/components/ui/button'
import { desktopClient } from '@/features/conversations/api/desktop-client'

interface GoalProgressProps {
  data: GoalProgressData
  className?: string
  defaultExpanded?: boolean
  onEvidenceClick?: (evidence: TaskEvidenceRecord) => void
  onResolveConfirmation?: (goalId: string, expectedVersion: number, approved: boolean) => Promise<boolean> | void
  onResolvePlanRevision?: (planRevisionId: string, decision: 'approved' | 'rejected') => Promise<boolean> | void
  onResolveWorkflowGate?: (workflowRunId: string, stageId: string, decision: 'approved' | 'rejected') => Promise<boolean> | void
}

export function GoalProgress({ data, className, defaultExpanded = false, onEvidenceClick, onResolveConfirmation, onResolvePlanRevision, onResolveWorkflowGate }: GoalProgressProps) {
  const [isExpanded, setIsExpanded] = useState(defaultExpanded)
  const [resolving, setResolving] = useState(false)
  const [resolvedWorkflowGateId, setResolvedWorkflowGateId] = useState<string | null>(null)
  const [workflowGateError, setWorkflowGateError] = useState<string | null>(null)
  const [cancelledTeamId, setCancelledTeamId] = useState<string | null>(null)
  const [teamError, setTeamError] = useState<string | null>(null)
  const { goal, tasks, evidence, planRevisions = [], reviewFindings = [], acceptances = [], expertWorkflow, expertTeam, completedCount, totalCount, currentTask } = data
  const pendingPlan = planRevisions
    .filter((plan) => plan.status === 'proposed')
    .sort((left, right) => right.revision - left.revision)[0]
  const pendingWorkflowGate = expertWorkflow?.gates.find((gate) => gate.status === 'pending')
  const currentWorkflowStage = expertWorkflow?.stages.find((stage) => stage.ordinal === expertWorkflow.run.currentStageIndex)
  const currentTeamMember = expertTeam?.members.find((member) => member.status === 'queued' || member.status === 'running')
  const terminalTeamMembers = expertTeam?.members.filter((member) => ['completed', 'failed', 'cancelled', 'interrupted'].includes(member.status)).length ?? 0
  const resolveWorkflowGate = async (decision: 'approved' | 'rejected') => {
    if (!expertWorkflow || !pendingWorkflowGate) return
    setResolving(true)
    setWorkflowGateError(null)
    try {
      if (onResolveWorkflowGate) {
        const resolved = await onResolveWorkflowGate(expertWorkflow.run.id, pendingWorkflowGate.stageId, decision)
        if (resolved === false) throw new Error('Workflow Gate 更新失败')
      } else {
        await desktopClient.resolveExpertWorkflowGate(expertWorkflow.run.id, pendingWorkflowGate.stageId, decision)
      }
      setResolvedWorkflowGateId(pendingWorkflowGate.id)
    } catch (cause) {
      setWorkflowGateError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setResolving(false)
    }
  }
  const cancelTeam = async () => {
    if (!expertTeam || expertTeam.run.status !== 'running') return
    setResolving(true)
    setTeamError(null)
    try {
      await desktopClient.cancelExpertTeam(expertTeam.run.id, '用户从工作进度卡停止专家团队')
      setCancelledTeamId(expertTeam.run.id)
    } catch (cause) {
      setTeamError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setResolving(false)
    }
  }

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
          <div className="fox-goal-confirmation">
            <p className="fox-goal-confirmation-copy">Fox 将为这项工作跟踪任务、执行过程和证据。是否进入工作模式并继续执行？</p>
            <div className="fox-goal-confirmation-actions">
              <Button
                variant="outline"
                disabled={resolving}
                onClick={async () => {
                  setResolving(true)
                  try { await onResolveConfirmation(goal.id, goal.version, false) } finally { setResolving(false) }
                }}
              >暂不执行</Button>
              <Button
                disabled={resolving}
                onClick={async () => {
                  setResolving(true)
                  try { await onResolveConfirmation(goal.id, goal.version, true) } finally { setResolving(false) }
                }}
              >继续执行</Button>
            </div>
          </div>
        )}

        {pendingPlan && onResolvePlanRevision && (
          <div className="border-t bg-card px-4 py-3">
            <div className="mb-2 flex items-center justify-between gap-3">
              <div>
                <div className="text-sm font-medium">计划 v{pendingPlan.revision} 待批准：{pendingPlan.title}</div>
                <p className="mt-1 text-xs text-muted-foreground">{pendingPlan.summary}</p>
              </div>
              <Badge variant="outline">待批准</Badge>
            </div>
            {pendingPlan.tasks.length > 0 && (
              <ol className="mb-3 list-decimal space-y-1 pl-5 text-xs text-muted-foreground">
                {pendingPlan.tasks.map((task, index) => {
                  const value = task && typeof task === 'object' ? task as Record<string, unknown> : {}
                  return <li key={`${pendingPlan.id}-${index}`}>{typeof value.title === 'string' ? value.title : `任务 ${index + 1}`}</li>
                })}
              </ol>
            )}
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={resolving}
                onClick={async () => {
                  setResolving(true)
                  try { await onResolvePlanRevision(pendingPlan.id, 'rejected') } finally { setResolving(false) }
                }}
              >退回修改</Button>
              <Button
                size="sm"
                disabled={resolving}
                onClick={async () => {
                  setResolving(true)
                  try { await onResolvePlanRevision(pendingPlan.id, 'approved') } finally { setResolving(false) }
                }}
              >批准计划</Button>
            </div>
          </div>
        )}

        {expertWorkflow && (
          <div className="border-t bg-card px-4 py-3">
            <div className="mb-2 flex items-center justify-between gap-3">
              <div>
                <div className="text-sm font-medium">专家工作流 {expertWorkflow.run.workflowId} · v{expertWorkflow.run.workflowVersion}</div>
                <p className="mt-1 text-xs text-muted-foreground">阶段 {expertWorkflow.run.currentStageIndex + 1}/{expertWorkflow.stages.length}：{currentWorkflowStage?.stageId ?? '未知'} · 尝试 {currentWorkflowStage?.attempt ?? 0}/{currentWorkflowStage?.maxAttempts ?? 0}</p>
              </div>
              <Badge variant={expertWorkflow.run.status === 'awaiting_gate' ? 'outline' : 'secondary'}>{expertWorkflow.run.status === 'awaiting_gate' ? '等待用户 Gate' : expertWorkflow.run.status}</Badge>
            </div>
            {pendingWorkflowGate && resolvedWorkflowGateId !== pendingWorkflowGate.id && <>
              <p className="mb-3 text-sm text-muted-foreground">下一阶段需要你的明确批准。拒绝会取消工作流并收口未完成任务。</p>
              <div className="flex justify-end gap-2">
                <Button variant="outline" size="sm" disabled={resolving} onClick={() => void resolveWorkflowGate('rejected')}>拒绝并停止</Button>
                <Button size="sm" disabled={resolving} onClick={() => void resolveWorkflowGate('approved')}>批准阶段</Button>
              </div>
            </>}
            {pendingWorkflowGate && resolvedWorkflowGateId === pendingWorkflowGate.id && (
              <p className="text-sm text-muted-foreground">Gate 决策已保存；下一次会话刷新将载入最新 Workflow 检查点。</p>
            )}
            {workflowGateError && <p className="mt-2 text-sm text-destructive">{workflowGateError}</p>}
          </div>
        )}

        {expertTeam && (
          <div className="border-t bg-card px-4 py-3">
            <div className="flex items-center justify-between gap-3">
              <div>
                <div className="text-sm font-medium">专家团队 {expertTeam.run.teamId} · v{expertTeam.run.teamVersion}</div>
                <p className="mt-1 text-xs text-muted-foreground">
                  已回传 {terminalTeamMembers}/{Array.isArray(expertTeam.run.team.members) ? expertTeam.run.team.members.length : expertTeam.members.length} 个成员
                  {currentTeamMember ? ` · 当前 ${currentTeamMember.teamMemberId ?? currentTeamMember.workerAgentName}` : ''}
                  {' · 串行 Supervisor'}
                </p>
              </div>
              <Badge variant={expertTeam.run.status === 'running' && cancelledTeamId !== expertTeam.run.id ? 'default' : 'secondary'}>
                {cancelledTeamId === expertTeam.run.id ? 'cancelled' : expertTeam.run.status}
              </Badge>
            </div>
            {expertTeam.run.status === 'running' && cancelledTeamId !== expertTeam.run.id && (
              <div className="mt-3 flex justify-end">
                <Button variant="outline" size="sm" disabled={resolving} onClick={() => void cancelTeam()}>停止团队</Button>
              </div>
            )}
            {teamError && <p className="mt-2 text-sm text-destructive">{teamError}</p>}
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
                <div><b>计划修订</b><span>{planRevisions.length ? `v${Math.max(...planRevisions.map((item) => item.revision))} · ${pendingPlan ? '待批准' : planRevisions.some((item) => item.status === 'approved') ? '已批准' : '未批准'}` : '未建立'}</span></div>
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
