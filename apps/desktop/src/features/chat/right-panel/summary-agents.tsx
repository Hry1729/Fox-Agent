import { AlertTriangle, Brain, Check, ChevronRight, Circle, CircleStop, LoaderCircle, Wrench } from 'lucide-react'
import { useEffect, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { ChildRunRecord, RunEventRecord } from '@/features/conversations/model/types'
import { FOX_ASSISTANT_AVATAR } from '../components/FoxAssistantAvatar'
import { presentRunActivity } from '../run-activity-presentation'
import { childRunIsActive, childRunStatusPresentation } from '../sidebar-model'

const activityEvents = new Set([
  'run.started', 'run.phase', 'run.completed', 'run.failed', 'run.cancelled', 'run.interrupted',
  'reasoning.delta', 'message.delta', 'tool.started', 'tool.completed', 'tool.failed',
  'user.question.requested', 'user.question.responded',
])

export function SummaryAgents({ agents, pendingAgentIds, onOpen }: { agents: ChildRunRecord[]; pendingAgentIds: ReadonlySet<string>; onOpen: () => void }) {
  const ordered = agents.toSorted((a, b) => Number(childRunIsActive(b)) - Number(childRunIsActive(a)) || b.createdAt - a.createdAt)
  return <>
    <ul className="fox-summary-list">
      {ordered.slice(0, 3).map(run => <SummaryAgent key={run.childRunId || run.id} run={run} awaitingApproval={pendingAgentIds.has(run.childConversationId)} />)}
    </ul>
    <button type="button" className="fox-side-text-button" onClick={onOpen}>查看全部 {agents.length} 条 <ChevronRight /></button>
  </>
}

function SummaryAgent({ run, awaitingApproval }: { run: ChildRunRecord; awaitingApproval: boolean }) {
  const [events, setEvents] = useState<RunEventRecord[]>([])
  const [preview, setPreview] = useState<{ seq: number; kind: 'answer' | 'reasoning' }>()
  const { childRunId, childConversationId, status } = run

  useEffect(() => {
    if (status !== 'running' || !desktopRuntimeAvailable) return
    let disposed = false
    const subscriptions = Promise.allSettled([
      desktopClient.listenRuntimeEvents(({ runId, seq, timestamp, event }) => {
        if (disposed || runId !== childRunId || !activityEvents.has(event.type)) return
        setEvents(previous => {
          if (event.type.endsWith('.delta') && previous.at(-1)?.eventType === event.type) return previous
          return [...previous, { runId, seq, eventType: event.type, event: event.type.endsWith('.delta') ? { type: event.type } : event, createdAt: Date.parse(timestamp) }]
        })
      }),
      desktopClient.listenKernelModelPreviews(({ runId, checkpointSeq, text, reasoning }) => {
        if (disposed || runId !== childRunId) return
        const kind = text.trim() ? 'answer' : reasoning?.trim() ? 'reasoning' : null
        if (kind) setPreview(previous => previous?.seq === checkpointSeq && previous.kind === kind ? previous : { seq: checkpointSeq, kind })
      }),
    ])
    void desktopClient.loadConversation(childConversationId).then(detail => {
      if (!disposed) setEvents(previous => [...detail.runtimeEvents.filter(event => event.runId === childRunId && activityEvents.has(event.eventType)), ...previous])
    }).catch(() => {})
    return () => {
      disposed = true
      void subscriptions.then(results => {
        for (const result of results) if (result.status === 'fulfilled') result.value()
      })
    }
  }, [childConversationId, childRunId, status])

  const latestSeq = events.reduce((latest, event) => Math.max(latest, event.seq), 0)
  const activity = status === 'running' ? presentRunActivity(events, {
    runId: childRunId, state: status, startedAt: run.startedAt,
    currentPreview: preview && preview.seq >= latestSeq ? preview.kind : undefined,
  }) : null
  const callingTool = activity?.active && ['searching', 'radar', 'orbit', 'progress'].includes(activity.animation)
  const thinking = activity?.animation === 'thinking'
  const statusPresentation = childRunStatusPresentation(status)
  const label = awaitingApproval ? '等待批准' : callingTool ? '调用工具中' : thinking ? '思考中'
    : activity?.label === '正在处理' ? '运行中' : activity?.label ?? statusPresentation.label
  const tone = awaitingApproval ? 'warning' : statusPresentation.tone
  const Indicator = awaitingApproval ? AlertTriangle : status === 'completed' ? Check : tone === 'danger' ? AlertTriangle
    : status === 'cancelled' ? CircleStop : callingTool ? Wrench : thinking ? Brain : status === 'running' || status === 'cancelling' ? LoaderCircle : Circle

  return <li className="fox-summary-item fox-summary-agent">
    <div className="fox-summary-avatar">
      <img src={FOX_ASSISTANT_AVATAR} alt="" />
      <span className={`fox-summary-avatar-status is-${tone}`}><Indicator aria-hidden="true" /></span>
    </div>
    <span title={run.workerAgentName}>{run.workerAgentName}</span>
    <small>{label}</small>
  </li>
}
