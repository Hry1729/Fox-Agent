import { ChevronDown, ChevronRight, ListEnd, Square, Terminal } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { command, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { ConversationDetail, ToolCallRecord } from '@/features/conversations/model/types'
import { notify } from '@/features/notifications'
import { isCommandTool } from '../composer/fox-tool-classification'
import { formatDurationMs } from '../sidebar-model'
import { toolPartialOutputText } from '../timeline/fox-runtime-process-model'
import { useLiveNow } from '../timeline/fox-runtime-process-view'
import { RightPanelEmpty } from './right-panel-empty'

type CommandTask = {
  id: string
  kind: 'job' | 'tool'
  runId: string
  runtimeCallId: string | null
  command: string
  title: string | null
  cwd: string | null
  status: string
  startedAt: number
  finishedAt: number | null
  stoppable: boolean
  stopping: boolean
  errorMessage: string | null
}

const activeStates = new Set(['running', 'queued', 'paused'])
const completedStates = new Set(['completed', 'failed', 'cancelled', 'interrupted'])
const asRecord = (value: unknown) => value && typeof value === 'object' ? value as Record<string, unknown> : {}
const text = (value: unknown) => typeof value === 'string' ? value : ''

function recordedTask(tool: ToolCallRecord): CommandTask | null {
  if (!isCommandTool(tool.toolName) && !/^(bash|shell_command|exec_command|local_shell)$/i.test(tool.toolName)) return null
  if (tool.status !== 'running' && !completedStates.has(tool.status)) return null
  const input = asRecord(tool.input)
  if (tool.toolName === 'run_command' && input.action && input.action !== 'sync' &&
      !(input.action === 'start' && tool.status === 'failed' && !asRecord(asRecord(tool.result).details).jobId)) return null
  return {
    id: tool.id, kind: 'tool', runId: tool.runId, runtimeCallId: tool.runtimeToolCallId,
    command: text(input.command ?? input.cmd ?? input.commandLine), title: text(input.title) || null, cwd: text(input.cwd) || null,
    status: tool.status, startedAt: tool.startedAt, finishedAt: tool.completedAt,
    stoppable: false, stopping: false, errorMessage: tool.errorMessage,
  }
}

export function useBackgroundTasks(detail: ConversationDetail | null) {
  const conversationId = detail?.conversation.id
  const [snapshot, setSnapshot] = useState<{ conversationId: string; tasks: CommandTask[] }>()
  const [requestedStops, setRequestedStops] = useState<Set<string>>(() => new Set())
  useEffect(() => {
    if (!desktopRuntimeAvailable || !conversationId) return
    const id = conversationId
    let closed = false
    let timeout: ReturnType<typeof setTimeout>
    async function refresh() {
      try {
        const tasks = await command<CommandTask[]>('conversation_command_tasks', { conversationId: id }, true)
        if (!closed) setSnapshot({ conversationId: id, tasks })
      } catch {
        // Keep the last snapshot, or use tool records already loaded with the chat.
      } finally {
        if (!closed) timeout = setTimeout(refresh, 1500)
      }
    }
    void refresh()
    return () => { closed = true; clearTimeout(timeout) }
  }, [conversationId])

  const tasks = useMemo(() => {
    if (snapshot && snapshot.conversationId === conversationId) return snapshot.tasks
    return (detail?.toolCalls ?? []).map(recordedTask).filter(task => task !== null)
      .sort((a, b) => b.startedAt - a.startedAt)
  }, [conversationId, detail?.toolCalls, snapshot])

  async function stop(task: CommandTask) {
    setRequestedStops(current => new Set(current).add(task.id))
    try {
      await command<void>('conversation_command_task_stop', { conversationId, taskId: task.id, kind: task.kind }, true)
    } catch {
      setRequestedStops(current => { const next = new Set(current); next.delete(task.id); return next })
      notify.error('无法停止这条命令。请查看任务的最新状态。')
    }
  }

  const running = tasks.filter(task => activeStates.has(task.status))
    .map(task => requestedStops.has(task.id) ? { ...task, stopping: true } : task)
  const finished = tasks.filter(task => !activeStates.has(task.status))
    .sort((a, b) => (b.finishedAt ?? b.startedAt) - (a.finishedAt ?? a.startedAt))
  return { running, finished, stop }
}

function taskTitle(task: CommandTask) {
  return task.title || task.command || '命令行任务'
}

function TaskStatus({ task, now }: { task: CommandTask; now: number }) {
  const status = task.stopping && activeStates.has(task.status) ? '停止中' : ({
    queued: '等待运行', paused: '已暂停', failed: '失败', interrupted: '已中断', cancelled: '已取消',
  } as Record<string, string>)[task.status]
  const duration = Number.isFinite(task.startedAt) ? formatDurationMs((task.finishedAt ?? now) - task.startedAt) : ''
  return <small className={task.status === 'failed' ? 'is-error' : undefined} title={status && duration ? duration : undefined}>{status || duration}</small>
}

function StopTask({ task, stop }: { task: CommandTask; stop: (task: CommandTask) => void }) {
  return task.stoppable && <button type="button" className="fox-command-task-stop" disabled={task.stopping} aria-label={`停止命令 ${taskTitle(task)}`} title="停止此命令" onClick={() => stop(task)}><Square size={11} fill="currentColor" /></button>
}

export function BackgroundTaskSummary({ running, finished, stop, onOpen }: ReturnType<typeof useBackgroundTasks> & { onOpen: () => void }) {
  const now = useLiveNow(running.some(task => task.status === 'running'))
  return <>
    {running.map(task => <div className="fox-command-task-summary" key={task.id}>
      <button type="button" className="fox-summary-item fox-command-task-trigger" onClick={onOpen}>
        <Terminal aria-hidden="true" /><span title={taskTitle(task)}>{taskTitle(task)}</span><TaskStatus task={task} now={now} />
      </button>
      <StopTask task={task} stop={stop} />
    </div>)}
    {finished.length > 0 && <button type="button" className="fox-side-text-button" onClick={onOpen}><ListEnd />{finished.length} 个已结束 <ChevronRight /></button>}
  </>
}

function useTaskResult(conversationId: string, task: CommandTask, live: boolean, current: unknown) {
  const [stored, setStored] = useState<unknown>()
  const [error, setError] = useState(false)
  const fetchResult = live || current == null
  useEffect(() => {
    if (!desktopRuntimeAvailable || !fetchResult) return
    let closed = false
    let timeout: ReturnType<typeof setTimeout>
    const refresh = async () => {
      try {
        const result = await command<unknown>('conversation_command_task_output', { conversationId, taskId: task.id, kind: task.kind }, true)
        if (!closed) { setStored(result); setError(false) }
      } catch {
        if (!closed) setError(true)
      } finally {
        if (live && !closed) timeout = setTimeout(refresh, 1500)
      }
    }
    void refresh()
    return () => { closed = true; clearTimeout(timeout) }
  }, [conversationId, fetchResult, live, task.id, task.kind])
  return { result: live ? stored ?? current : current ?? stored, error }
}

function TaskOutput({ detail, task }: { detail: ConversationDetail; task: CommandTask }) {
  const call = task.kind === 'tool' ? detail.toolCalls.find(tool => tool.id === task.id) : undefined
  const live = activeStates.has(task.status)
  const { result, error } = useTaskResult(detail.conversation.id, task, live, call?.result)
  const record = asRecord(result)
  const details = asRecord(record.details ?? record.rawOutput ?? result)
  const partial = task.kind === 'tool' ? detail.runtimeEvents.findLast(event =>
    event.runId === task.runId && event.eventType === 'tool.updated' && event.event.toolCallId === task.runtimeCallId
      && toolPartialOutputText(event.event.update ?? event.event.output)) : undefined
  const stdout = text(details.stdout)
  const stderr = text(details.stderr)
  const output = [
    stdout, stderr && `stderr:\n${stderr}`,
  ].filter(Boolean).join('\n\n') || text(record.preview) || toolPartialOutputText(result) || toolPartialOutputText(record.rawOutput) ||
    toolPartialOutputText(partial?.event.update ?? partial?.event.output)
  return <div className="fox-command-task-content">
    {task.command && <pre className="fox-command-task-code">{`$ ${task.command}`}</pre>}
    {(task.cwd || typeof details.exitCode === 'number') && <p className="fox-command-task-meta">{[task.cwd, typeof details.exitCode === 'number' ? `退出码 ${details.exitCode}` : null].filter(Boolean).join(' · ')}</p>}
    <div className="fox-command-task-output">
      {output ? <pre className="fox-command-task-code">{output}</pre> : !task.errorMessage && <p>{error ? '无法读取命令输出。' : result === undefined && desktopRuntimeAvailable ? '正在读取输出…' : '暂无输出'}</p>}
      {task.errorMessage && <p className="is-error">{task.errorMessage}</p>}
      {record.truncated === true && <p>此处显示已保留的输出。</p>}
    </div>
  </div>
}

function TaskRow({ task, detail, now, stop, open, onToggle }: { task: CommandTask; detail: ConversationDetail; now: number; stop: (task: CommandTask) => void; open: boolean; onToggle: () => void }) {
  return <li className="fox-command-task">
    <div className="fox-command-task-header">
      <button type="button" className="fox-command-task-trigger" aria-expanded={open} onClick={onToggle}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}<span title={taskTitle(task)}>{taskTitle(task)}</span><TaskStatus task={task} now={now} />
      </button>
      <StopTask task={task} stop={stop} />
    </div>
    {open && <TaskOutput detail={detail} task={task} />}
  </li>
}

export function BackgroundTasksPanel({ detail }: { detail: ConversationDetail | null }) {
  const { running, finished, stop } = useBackgroundTasks(detail)
  const now = useLiveNow(running.some(task => task.status === 'running'))
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set())
  if (!detail || !running.length && !finished.length) return <RightPanelEmpty icon={Terminal}>暂无命令行任务。</RightPanelEmpty>
  const toggleTask = (id: string) => setExpanded(current => {
    const next = new Set(current)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    return next
  })
  const taskList = (tasks: CommandTask[]) => <ul>{tasks.map(task => <TaskRow key={task.id} task={task} detail={detail} now={now} stop={stop} open={expanded.has(task.id)} onToggle={() => toggleTask(task.id)} />)}</ul>
  return <div className="fox-command-tasks">
    {running.length > 0 && <section aria-label="运行中的后台任务"><h2>正在运行 {running.length}</h2>{taskList(running)}</section>}
    {finished.length > 0 && <details open className="fox-command-finished">
      <summary><ChevronDown size={14} />已结束 {finished.length}</summary>
      {taskList(finished)}
    </details>}
  </div>
}
