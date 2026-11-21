import { useEffect, useState } from 'react'
import { Activity, AlertTriangle, Pause, Play, Trash2, CircleStop, LoaderCircle } from 'lucide-react'
import { SideCard, SideSection } from '@/components/fox-ui/side-section'
import { Progress } from '@/components/ui/progress'
import { FoxDisclosure } from '@/components/fox-ui/fox-disclosure'
import { useDeveloperUi } from '@/features/chat/developer-ui'
import { userFacingError } from '@/features/chat/send-failure-notice'
import type { ApprovalRecord, ConversationDetail, TaskEvidenceRecord, ToolCallRecord } from '@/features/conversations/model/types'
import { toolCallApprovalNote } from '@/features/conversations/model/approval-labels'
import type { GoalProgressData } from '../hooks/use-goal-progress'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { notify as toast } from '@/features/notifications'
import { formatDurationMs } from '../sidebar-model'
import { toolRecordPresentation } from '../tool-record-presentation'
import { PermissionAuditList } from './PermissionAuditList'
import { RightPanelEmpty } from '../right-panel/right-panel-empty'

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
  if (['awaiting_approval', 'waiting_approval'].includes(status)) return '需要你批准'
  if (['awaiting_question', 'waiting_question'].includes(status)) return '需要你回答'
  if (status === 'completed') return '已完成'
  if (status === 'failed') return '失败'
  if (['cancelled', 'canceled'].includes(status)) return '已取消'
  if (status === 'interrupted') return '已中断'
  return status
}

const evidenceStatus = { unverified: '待核验', valid: '有效', stale: '已过期', missing: '缺失', invalid: '无效' } as const

function sourceTool(detail: ConversationDetail | null, evidence: TaskEvidenceRecord | null) {
  if (evidence?.refKind !== 'tool_call') return null
  return detail?.toolCalls.find((tool) => tool.id === evidence.refId || tool.runtimeToolCallId === evidence.refId) ?? null
}

function ToolEntry({ tool, focused, approvals, developerUi }: { tool: ToolCallRecord; focused: boolean; approvals?: ApprovalRecord[]; developerUi: boolean }) {
  const [open, setOpen] = useState(focused)
  useEffect(() => { if (focused) setOpen(true) }, [focused])
  const { action, target, Icon } = toolRecordPresentation(tool)
  const note = toolCallApprovalNote(tool, approvals)
  return <div id={`fox-activity-tool-${tool.id}`} className={`fox-tool-entry ${focused ? 'is-focused' : ''}`}>
    <button type="button" className="fox-record-row" aria-expanded={open} onClick={() => setOpen(value => !value)}>
      <Icon /><span className="fox-record-copy"><strong>{action}</strong><small title={target}>{target}</small>{developerUi && <small>{tool.toolName}</small>}</span>
      <small className={`fox-side-status is-${tool.status}`}>{toolStatus(tool)}</small>
    </button>
    {open && <div className="fox-record-result">{note && <p className="fox-activity-warning">{note}</p>}{developerUi && <pre>{JSON.stringify({ input: tool.input, result: tool.result }, null, 2).slice(0, 3000)}</pre>}</div>}
  </div>
}

export function ActivityDetailPanel({ detail, goal, focusedEvidence, onEvidenceClick, onDeleteGoal, onGoalRunningChange }: ActivityDetailPanelProps) {
  const developerUi = useDeveloperUi()
  const [busy, setBusy] = useState(false)
  const [showAllTools, setShowAllTools] = useState(false)
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
      await desktopClient.cancelExpertTeam(team.id, '用户从相关依据停止专家团队')
      toast.success('已请求停止专家团队')
    } catch (cause) {
      toast.error(userFacingError(cause instanceof Error ? cause.message : String(cause), developerUi, '没有停止成功，请重试。'))
    } finally { setBusy(false) }
  }

  if (!detail?.lastRun && !goal && !focusedEvidence && !tools.length && !recentEvents.length) {
    return <RightPanelEmpty icon={Activity}>这段对话还没有运行记录。</RightPanelEmpty>
  }

  return <div className="fox-activity-detail">
    {detail?.lastRun && <SideSection title="最近一次运行"><SideCard><div className="fox-record-row fox-activity-overview">
      <Play /><span className="fox-record-copy"><strong>{runStatus(detail.lastRun.status)} · {detail.toolCalls.length} 步</strong>
      {detail.lastRun.errorMessage ? <small className="is-error">{userFacingError(detail.lastRun.errorMessage, developerUi, '这次运行没有完成。')}</small> : <small>{detail.lastRun.startedAt ? formatDurationMs((detail.lastRun.finishedAt ?? Date.now()) - detail.lastRun.startedAt) : '—'}</small>}</span>
      {['running', 'streaming', 'awaiting_approval', 'awaiting_question'].includes(detail.lastRun.status) && <button type="button" className="fox-panel-icon-button is-small" aria-label="停止运行" onClick={() => { void desktopClient.cancelRun(detail.lastRun!.id).catch(() => toast.error('停止失败，请重试。')) }}><CircleStop /></button>}
    </div></SideCard></SideSection>}
    {focusedEvidence && <SideSection title="相关依据"><SideCard className="fox-activity-evidence-focus"><p>{focusedEvidence.summary}</p><small>核验状态：{evidenceStatus[focusedEvidence.validityStatus]}{matchedTool ? ` · ${developerUi ? `${matchedTool.toolName} · ` : ''}${toolStatus(matchedTool)}` : focusedEvidence.refKind === 'run_event' ? ' · 运行记录见下方' : ' · 原始记录暂不可直接定位'}</small></SideCard></SideSection>}
    {goal && <SideSection title="目标"><SideCard className="fox-activity-goal">
      <div className="fox-activity-section-head"><div><strong>{goal.goal.title}</strong><small>{goalStatus(goal.goal.status)} · {goal.completedCount}/{goal.totalCount} 项任务完成</small></div>
        <div className="fox-activity-actions">
          {onGoalRunningChange && ['active', 'blocked'].includes(goal.goal.status) && <button type="button" className="fox-side-text-button" disabled={busy} onClick={() => void changeGoalRunning()}>{goal.goal.status === 'active' ? <Pause size={13} /> : <Play size={13} />}{goal.goal.status === 'active' ? '暂停' : '继续'}</button>}
          {onDeleteGoal && <button type="button" className="fox-side-text-button" disabled={busy} onClick={() => void deleteGoal()}><Trash2 size={13} />删除目标</button>}
        </div>
      </div>
      <Progress value={goal.totalCount ? goal.completedCount / goal.totalCount * 100 : 0} aria-label="目标进度" />
      {goal.goal.blockedReason && <p className="fox-activity-warning">{goal.goal.blockedReason}</p>}
      {team && <p className="fox-side-note">专家团队 · {runStatus(team.status)}</p>}
      {team?.status === 'running' && <button type="button" className="fox-activity-team-stop" disabled={busy} onClick={() => void cancelTeam()}>停止协作</button>}
      <FoxDisclosure summary="查看计划、任务和证据">
        {goal.planRevisions.map((plan) => <p key={plan.id}><strong>计划 v{plan.revision}</strong>{developerUi ? ` · ${plan.status}` : ''} · {plan.title}</p>)}
        {goal.tasks.map((task) => <div className="fox-activity-task" key={task.id}><strong>{task.title}</strong><small>{developerUi ? task.status : '任务'}{task.blockedReason ? ` · ${userFacingError(task.blockedReason, developerUi, '这一项被挡住了。')}` : ''}</small>{goal.evidence.filter((evidence) => evidence.taskId === task.id).map((evidence) => <button type="button" key={evidence.id} onClick={() => onEvidenceClick?.(evidence)}>{evidence.summary}{developerUi && <small>{evidence.validityStatus}</small>}</button>)}</div>)}
        {goal.acceptances.length > 0 && <p>验收记录：{goal.acceptances.length} 条。</p>}
      </FoxDisclosure>
    </SideCard></SideSection>}
    <SideSection title="工具记录" count={detail?.toolCalls.length ?? 0}><SideCard className="fox-activity-tools">
      {tools.length === 0 ? <p className="fox-activity-empty">当前没有工具记录。</p> : (showAllTools || matchedTool ? tools : tools.slice(-4)).map((tool) => <ToolEntry key={tool.id} tool={tool} focused={matchedTool?.id === tool.id} approvals={detail?.approvals} developerUi={developerUi} />)}
      {!matchedTool && tools.length > 4 && <button type="button" className="fox-side-more" aria-expanded={showAllTools} onClick={() => setShowAllTools(value => !value)}>{showAllTools ? '收起' : `还有 ${tools.length - 4} 条 ›`}</button>}
      {(detail?.toolCalls.length ?? 0) > tools.length && <small>仅显示最近 100 条；更早的记录可在对话历史中查看。</small>}
    </SideCard></SideSection>
    <PermissionAuditList runId={detail?.lastRun?.id} />
    {developerUi && <SideSection title="运行记录" count={detail?.runtimeEvents.length ?? 0}><SideCard className="fox-activity-events">
      {recentEvents.length === 0 ? <p className="fox-activity-empty">当前没有运行记录。</p> : developerUi ? recentEvents.map((event) => <p key={`${event.runId}:${event.seq}`}><span>{event.eventType}</span><small>{new Date(event.createdAt).toLocaleTimeString()}</small></p>) : <p className="fox-activity-empty">共 {recentEvents.length} 条。事件名只在开发者模式显示。</p>}
      {(detail?.runtimeEvents.length ?? 0) > recentEvents.length && <small>仅显示最近 30 条。</small>}
    </SideCard></SideSection>}
  </div>
}
