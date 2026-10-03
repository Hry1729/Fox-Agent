import { useEffect, useState } from 'react'
import { Activity, AlertTriangle, Pause, Play, Trash2 } from 'lucide-react'
import type { ConversationDetail, TaskEvidenceRecord, ToolCallRecord } from '@/features/conversations/model/types'
import type { GoalProgressData } from '../hooks/use-goal-progress'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { notify as toast } from '@/features/notifications'

export interface ActivityDetailPanelProps {
  detail: ConversationDetail | null
  goal: GoalProgressData | null
  focusedEvidence: TaskEvidenceRecord | null
  onEvidenceClick?: (evidence: TaskEvidenceRecord) => void
  onDeleteGoal?: (goalId: string) => Promise<boolean>
  onGoalRunningChange?: (goalId: string, expectedVersion: number, running: boolean) => Promise<boolean>
}

function toolStatus(tool: ToolCallRecord) {
  if (tool.status === 'completed') return '已完成'
  if (tool.status === 'failed') return '失败'
  if (tool.status === 'denied') return '已拒绝'
  if (tool.status === 'cancelled') return '已取消'
  if (tool.status === 'pending') return '等待处理'
  return '进行中'
}

function goalStatus(status: string) {
  return status === 'active' ? '进行中'
    : status === 'blocked' ? '已暂停或阻塞'
      : status === 'completed' ? '已完成'
        : status === 'proposed' ? '等待确认' : status
}

function runStatus(status: string) {
  if (['running', 'streaming'].includes(status)) return '进行中'
  if (['queued', 'pending'].includes(status)) return '排队中'
  if (['awaiting_approval', 'waiting_approval'].includes(status)) return '等待批准'
  if (['awaiting_question', 'waiting_question'].includes(status)) return '等待回答'
  if (status === 'completed') return '已完成'
  if (status === 'failed') return '失败'
  if (['cancelled', 'canceled'].includes(status)) return '已取消'
  if (status === 'interrupted') return '已中断'
  return status
}

function sourceTool(detail: ConversationDetail | null, evidence: TaskEvidenceRecord | null) {
  if (evidence?.refKind !== 'tool_call') return null
  return detail?.toolCalls.find((tool) => tool.id === evidence.refId || tool.runtimeToolCallId === evidence.refId) ?? null
}

function ToolEntry({ tool, focused }: { tool: ToolCallRecord; focused: boolean }) {
  const [open, setOpen] = useState(focused)
  useEffect(() => { if (focused) setOpen(true) }, [focused])
  return <details id={`fox-activity-tool-${tool.id}`} className={focused ? 'is-focused' : ''} open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary><span>{tool.toolName}</span><small>{toolStatus(tool)}</small></summary>
    {open && <>{tool.errorMessage && <p className="fox-activity-warning">{tool.errorMessage}</p>}<pre>{JSON.stringify({ input: tool.input, result: tool.result }, null, 2).slice(0, 3000)}</pre></>}
  </details>
}

export function ActivityDetailPanel({ detail, goal, focusedEvidence, onEvidenceClick, onDeleteGoal, onGoalRunningChange }: ActivityDetailPanelProps) {
  const [busy, setBusy] = useState(false)
  const matchedTool = sourceTool(detail, focusedEvidence)
  const recentTools = detail?.toolCalls.slice(-100) ?? []
  const tools = matchedTool && !recentTools.includes(matchedTool) ? [matchedTool, ...recentTools] : recentTools
  const recentEvents = detail?.runtimeEvents.slice(-30) ?? []
  const team = goal?.expertTeam?.run

  useEffect(() => {
    if (!matchedTool) return
    const timer = window.setTimeout(() => document.getElementById(`fox-activity-tool-${matchedTool.id}`)?.scrollIntoView({ block: 'center' }), 120)
    return () => window.clearTimeout(timer)
  }, [matchedTool?.id])

  const changeGoalRunning = async () => {
    if (!goal || !onGoalRunningChange || busy) return
    setBusy(true)
    try {
      const changed = await onGoalRunningChange(goal.goal.id, goal.goal.version, goal.goal.status !== 'active')
      if (!changed) toast.error('目标状态更新失败')
    } finally { setBusy(false) }
  }
  const deleteGoal = async () => {
    if (!goal || !onDeleteGoal || busy) return
    setBusy(true)
    try {
      if (!await onDeleteGoal(goal.goal.id)) toast.error('目标删除失败，请先停止当前生成后重试')
    } finally { setBusy(false) }
  }
  const cancelTeam = async () => {
    if (!team || team.status !== 'running' || busy) return
    setBusy(true)
    try {
      await desktopClient.cancelExpertTeam(team.id, '用户从运行详情停止专家团队')
      toast.success('已请求停止专家团队')
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause))
    } finally { setBusy(false) }
  }

  return <div className="fox-activity-detail">
    <header><Activity size={17} /><div><strong>运行详情</strong><small>按需查看执行、计划和证据</small></div></header>
    {detail?.lastRun && <section className="fox-activity-overview"><strong>最近一次运行</strong><span>{runStatus(detail.lastRun.status)}</span>{detail.lastRun.errorMessage && <p><AlertTriangle size={13} />{detail.lastRun.errorMessage}</p>}</section>}
    {focusedEvidence && <section className="fox-activity-evidence-focus"><strong>所选证据</strong><p>{focusedEvidence.summary}</p><small>核验状态：{focusedEvidence.validityStatus}{matchedTool ? ` · ${matchedTool.toolName} · ${toolStatus(matchedTool)}` : focusedEvidence.refKind === 'run_event' ? ' · 运行记录见下方' : ' · 原始记录暂不可直接定位'}</small></section>}
    {goal && <section className="fox-activity-goal">
      <div className="fox-activity-section-head"><div><strong>{goal.goal.title}</strong><small>{goalStatus(goal.goal.status)} · {goal.completedCount}/{goal.totalCount} 项任务完成</small></div>
        <div className="fox-activity-actions">
          {onGoalRunningChange && ['active', 'blocked'].includes(goal.goal.status) && <button type="button" disabled={busy} onClick={() => void changeGoalRunning()}>{goal.goal.status === 'active' ? <Pause size={13} /> : <Play size={13} />}{goal.goal.status === 'active' ? '暂停' : '继续'}</button>}
          {onDeleteGoal && <button type="button" disabled={busy} onClick={() => void deleteGoal()}><Trash2 size={13} />删除目标</button>}
        </div>
      </div>
      {goal.goal.blockedReason && <p className="fox-activity-warning">{goal.goal.blockedReason}</p>}
      {team?.status === 'running' && <button type="button" className="fox-activity-team-stop" disabled={busy} onClick={() => void cancelTeam()}>停止专家团队</button>}
      <details><summary>查看计划、任务和证据</summary>
        {goal.planRevisions.map((plan) => <p key={plan.id}><strong>计划 v{plan.revision}</strong> · {plan.status} · {plan.title}</p>)}
        {goal.tasks.map((task) => <div className="fox-activity-task" key={task.id}><strong>{task.title}</strong><small>{task.status}{task.blockedReason ? ` · ${task.blockedReason}` : ''}</small>{goal.evidence.filter((evidence) => evidence.taskId === task.id).map((evidence) => <button type="button" key={evidence.id} onClick={() => onEvidenceClick?.(evidence)}>{evidence.summary}<small>{evidence.validityStatus}</small></button>)}</div>)}
        {goal.acceptances.length > 0 && <p>验收记录：{goal.acceptances.length} 条。任务完成不代表验收通过。</p>}
      </details>
    </section>}
    <section className="fox-activity-tools"><div className="fox-activity-section-head"><strong>工具与后台活动</strong><small>{detail?.toolCalls.length ?? 0} 条</small></div>
      {tools.length === 0 ? <p className="fox-activity-empty">当前没有工具记录。</p> : tools.map((tool) => <ToolEntry key={tool.id} tool={tool} focused={matchedTool?.id === tool.id} />)}
      {(detail?.toolCalls.length ?? 0) > tools.length && <small>仅显示最近 100 条；更早的记录可在会话历史中查看。</small>}
    </section>
    <section className="fox-activity-events"><div className="fox-activity-section-head"><strong>运行记录</strong><small>{detail?.runtimeEvents.length ?? 0} 条</small></div>
      {recentEvents.length === 0 ? <p className="fox-activity-empty">当前没有运行记录。</p> : recentEvents.map((event) => <p key={`${event.runId}:${event.seq}`}><span>{event.eventType}</span><small>{new Date(event.createdAt).toLocaleTimeString()}</small></p>)}
      {(detail?.runtimeEvents.length ?? 0) > recentEvents.length && <small>仅显示最近 30 条。</small>}
    </section>
  </div>
}
