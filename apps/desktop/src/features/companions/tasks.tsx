import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { CalendarClock, History, Pencil, Play, Plus, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { command } from '@/features/conversations/api/desktop-client'
import type { DigitalColleagueRecord, DigitalColleagueScheduleRecord, DigitalColleagueTriggerRecord } from '@/features/conversations/model/types'
import { showContactError } from './context'

export type CompanionSchedule =
  | { kind: 'once'; atMs: number; timezone: string }
  | { kind: 'interval'; intervalSeconds: number; timezone: string }
  | { kind: 'daily' | 'weekly'; weekdays: number[]; localTime: string; timezone: string }
export type OwnedCompanionTask = { task: DigitalColleagueRecord; schedule: DigitalColleagueScheduleRecord }
type TaskHistory = Record<string, DigitalColleagueTriggerRecord[]>
type EditorTask = OwnedCompanionTask | 'new' | null
type ScheduleKind = CompanionSchedule['kind']

const WEEKDAYS = [['日', 0], ['一', 1], ['二', 2], ['三', 3], ['四', 4], ['五', 5], ['六', 6]] as const
const ALL_WEEKDAYS = WEEKDAYS.map(([, value]) => value)
const STATUS_LABELS: Record<DigitalColleagueTriggerRecord['status'], string> = {
  accepted: '待派发', queued: '排队中', running: '执行中', completed: '已完成',
  failed: '失败', cancelled: '已取消', rejected: '已拒绝', skipped: '已跳过',
}
const ACTIVE_STATUSES: ReadonlyArray<DigitalColleagueTriggerRecord['status']> = ['accepted', 'queued', 'running']

function activeTrigger(runs: DigitalColleagueTriggerRecord[] | undefined) {
  return runs?.find(run => ACTIVE_STATUSES.includes(run.status))
}

function intervalParts(seconds: number | null | undefined): { value: string; unit: '1' | '60' | '3600' | '86400' } {
  if (!seconds || seconds <= 0) return { value: '2', unit: '3600' }
  if (seconds % 86_400 === 0) return { value: String(seconds / 86_400), unit: '86400' }
  if (seconds % 3_600 === 0) return { value: String(seconds / 3_600), unit: '3600' }
  if (seconds % 60 === 0) return { value: String(seconds / 60), unit: '60' }
  return { value: String(seconds), unit: '1' }
}

function listTasks(companionId: string) {
  return command<OwnedCompanionTask[]>('companion_task_list', { companionId })
}
function taskHistory(companionId: string, taskId: string, limit = 20) {
  return command<DigitalColleagueTriggerRecord[]>('companion_task_history', { companionId, taskId, limit })
}
function taskCommand<T>(name: string, companionId: string, taskId: string) {
  return command<T>(name, { companionId, taskId })
}

export function useCompanionTasks(companionId: string, active: boolean) {
  const [tasks, setTasks] = useState<OwnedCompanionTask[]>([])
  const [history, setHistory] = useState<TaskHistory>({})
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState('')
  const generation = useRef(0)
  const refresh = useCallback(async () => {
    const current = ++generation.current
    setError('')
    try {
      const saved = await listTasks(companionId)
      const entries = await Promise.all(saved.map(async owned => [owned.task.id, await taskHistory(companionId, owned.task.id)] as const))
      if (generation.current !== current) return
      setTasks(saved)
      setHistory(Object.fromEntries(entries))
      setLoaded(true)
    } catch (cause) {
      if (generation.current === current) setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [companionId])
  useEffect(() => {
    setLoaded(false)
    setTasks([])
    setHistory({})
    if (!active) return () => { generation.current += 1 }
    void refresh()
    const timer = window.setInterval(() => void refresh(), 5000)
    return () => {
      generation.current += 1
      window.clearInterval(timer)
    }
  }, [active, refresh])
  const pending = tasks.filter(owned => Boolean(activeTrigger(history[owned.task.id]))).length
  const due = tasks.filter(owned => owned.schedule.enabled && owned.schedule.nextDueAt <= Date.now()).length
  return { tasks, history, loaded, error, pending, due, refresh }
}

function deviceTimezone() {
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone
  if (!timezone) throw new Error('无法确定设备时区，请先设置设备时区。')
  return timezone
}

function dateParts(at: number, timezone: string) {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone: timezone, year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', hourCycle: 'h23',
  }).formatToParts(at)
  const values = Object.fromEntries(parts.map(part => [part.type, part.value]))
  return { year: Number(values.year), month: Number(values.month), day: Number(values.day), hour: Number(values.hour), minute: Number(values.minute) }
}

function localInputValue(at: number, timezone: string) {
  const value = dateParts(at, timezone)
  return `${value.year}-${String(value.month).padStart(2, '0')}-${String(value.day).padStart(2, '0')}T${String(value.hour).padStart(2, '0')}:${String(value.minute).padStart(2, '0')}`
}

function wallClockMs(value: string, timezone: string) {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value)
  if (!match) throw new Error('请选择明确的一次性执行时间。')
  const [, year, month, day, hour, minute] = match
  const wall = Date.UTC(Number(year), Number(month) - 1, Number(day), Number(hour), Number(minute))
  const candidates = new Map<number, number>()
  for (const guess of [wall - 86_400_000, wall, wall + 86_400_000]) {
    const zoned = dateParts(guess, timezone)
    const represented = Date.UTC(zoned.year, zoned.month - 1, zoned.day, zoned.hour, zoned.minute)
    const instant = wall - (represented - guess)
    const actual = dateParts(instant, timezone)
    if (Date.UTC(actual.year, actual.month - 1, actual.day, actual.hour, actual.minute) === wall) candidates.set(instant, instant)
  }
  if (candidates.size === 0) throw new Error('这个时间在该时区不存在，请选择其他时间。')
  if (candidates.size > 1) throw new Error('这个时间在该时区出现两次，请选择其他明确时间。')
  return [...candidates.keys()][0]
}

function formatAt(at: number | null | undefined, timezone: string) {
  if (at == null || !Number.isFinite(at)) return '暂无'
  return new Intl.DateTimeFormat('zh-CN', { timeZone: timezone, month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(at)
}

function scheduleText(schedule: DigitalColleagueScheduleRecord) {
  if (schedule.scheduleKind === 'once') return `单次 · ${formatAt(schedule.onceAt, schedule.timezone)}`
  if (schedule.scheduleKind === 'interval') {
    const seconds = schedule.intervalSeconds ?? 0
    if (seconds % 86_400 === 0) return `每 ${seconds / 86_400} 天`
    if (seconds % 3_600 === 0) return `每 ${seconds / 3_600} 小时`
    if (seconds % 60 === 0) return `每 ${seconds / 60} 分钟`
    return `每 ${seconds} 秒`
  }
  const days = schedule.scheduleKind === 'daily' ? '每天' : `每周${(schedule.weekdays ?? []).map(day => WEEKDAYS.find(([, value]) => value === day)?.[0] ?? '').join('、')}`
  return `${days} ${schedule.localTime ?? ''}`
}

function latestResult(trigger: DigitalColleagueTriggerRecord | undefined) {
  if (!trigger) return '尚未运行'
  const status = STATUS_LABELS[trigger.status]
  return trigger.errorMessage ? `${status} · ${trigger.errorMessage}` : status
}

export function ContactTaskSummary({ count, pending, due, loaded }: { count: number; pending: number; due: number; loaded: boolean }) {
  if (!loaded) return <span className="fox-contact-side-sub">读取中…</span>
  if (!count) return <span className="fox-contact-side-sub">暂无任务</span>
  return <span className="fox-contact-side-sub">{count} 个任务{pending > 0 ? ` · ${pending} 个待处理` : due > 0 ? ` · ${due} 个已到期` : ''}</span>
}

export function ContactTasks({ companionId, tasks, history, refresh, error }: {
  companionId: string
  tasks: OwnedCompanionTask[]
  history: TaskHistory
  refresh: () => Promise<void>
  error?: string
}) {
  const [editor, setEditor] = useState<EditorTask>(null)
  const [historyTask, setHistoryTask] = useState<string | null>(null)
  const [pendingDelete, setPendingDelete] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const act = async (action: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await action()
      await refresh()
      return true
    } catch (error) {
      showContactError(error)
      return false
    } finally { setBusy(false) }
  }
  return <div className="fox-contact-tasks">
    {error && <p className="fox-contact-task-meta" role="alert">{error}</p>}
    <Button size="sm" variant="outline" disabled={busy} onClick={() => setEditor('new')}><Plus size={14} />添加任务</Button>
    {editor && <TaskEditor key={editor === 'new' ? 'new' : editor.task.id} companionId={companionId} owned={editor === 'new' ? null : editor}
      busy={busy} onCancel={() => setEditor(null)} onSave={async request => {
        setBusy(true)
        try {
          if (editor === 'new') await command('companion_task_create', { companionId, ...request })
          else await command('companion_task_update', { companionId, taskId: editor.task.id, ...request })
          await refresh()
          setEditor(null)
        } catch (error) {
          showContactError(error)
          throw error
        } finally { setBusy(false) }
      }} />}
    {!tasks.length && !editor && <p className="fox-contact-task-empty">还没有定时任务。</p>}
    {tasks.map(owned => {
      const id = owned.task.id
      const runs = history[id] ?? []
      const latest = runs[0]
      const active = activeTrigger(runs)
      return <article className="fox-contact-task" key={id} aria-label={owned.task.name}>
        <div className="fox-contact-task-title"><CalendarClock size={15} /><strong>{owned.task.name}</strong></div>
        <p>{owned.task.objective}</p>
        <p className="fox-contact-task-meta">{scheduleText(owned.schedule)} · {owned.schedule.enabled ? '已启用' : '已暂停'}</p>
        <p className="fox-contact-task-meta">下次执行：{owned.schedule.enabled ? formatAt(owned.schedule.nextDueAt, owned.schedule.timezone) : '已暂停'} · {owned.schedule.timezone}</p>
        {active && active.id !== latest?.id && <p className={`fox-contact-task-result${active.status === 'queued' ? ' is-queued' : ''}`}>当前状态：{STATUS_LABELS[active.status]}</p>}
        <p className={`fox-contact-task-result${latest?.status === 'queued' ? ' is-queued' : ''}`}>最近结果：{latestResult(latest)}</p>
        <div className="fox-contact-task-actions">
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => setEditor(owned)}><Pencil size={13} />编辑</Button>
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => void act(() => taskCommand(owned.schedule.enabled ? 'companion_task_pause' : 'companion_task_resume', companionId, id))}>{owned.schedule.enabled ? '暂停' : '启用'}</Button>
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => void act(() => taskCommand('companion_task_run_now', companionId, id))}><Play size={13} />立即执行</Button>
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => setHistoryTask(historyTask === id ? null : id)}><History size={13} />运行记录</Button>
          {pendingDelete === id ? <>
            <span className="fox-contact-task-meta">删除后历史产物会保留。</span>
            <Button size="sm" variant="destructive" disabled={busy} onClick={() => void act(() => taskCommand('companion_task_delete', companionId, id)).then(saved => { if (saved) setPendingDelete(null) })}><Trash2 size={13} />确认删除</Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setPendingDelete(null)}>取消</Button>
          </> : <Button size="sm" variant="ghost" disabled={busy} onClick={() => setPendingDelete(id)}><Trash2 size={13} />删除</Button>}
        </div>
        {historyTask === id && <TaskRuns owned={owned} runs={history[id] ?? []} />}
      </article>
    })}
  </div>
}

function TaskRuns({ owned, runs }: { owned: OwnedCompanionTask; runs: DigitalColleagueTriggerRecord[] }) {
  return <section className="fox-contact-task-runs" aria-label={`${owned.task.name} 运行记录`}>
    <h3>运行记录</h3>
    {!runs.length ? <p>暂无运行记录。</p> : runs.map(run => <div className="fox-contact-task-run" key={run.id}>
      <span>{formatAt(run.scheduledFor ?? run.createdAt, owned.schedule.timezone)}</span>
      <strong>{STATUS_LABELS[run.status]}</strong>
      {run.errorMessage && <p>{run.errorMessage}</p>}
    </div>)}
  </section>
}

function TaskEditor({ companionId, owned, busy, onCancel, onSave }: {
  companionId: string
  owned: OwnedCompanionTask | null
  busy: boolean
  onCancel: () => void
  onSave: (request: { name: string; objective: string; schedule: CompanionSchedule; enabled: boolean }) => Promise<void>
}) {
  const timezone = useMemo(() => owned?.schedule.timezone ?? deviceTimezone(), [owned])
  const [name, setName] = useState(owned?.task.name ?? '')
  const [objective, setObjective] = useState(owned?.task.objective ?? '')
  const [kind, setKind] = useState<ScheduleKind>(owned?.schedule.scheduleKind ?? 'interval')
  const originalOnceAt = owned?.schedule.onceAt ?? null
  const [onceAt, setOnceAt] = useState(originalOnceAt != null ? localInputValue(originalOnceAt, timezone) : '')
  const initialInterval = intervalParts(owned?.schedule.intervalSeconds)
  const [intervalValue, setIntervalValue] = useState(initialInterval.value)
  const [intervalUnit, setIntervalUnit] = useState(initialInterval.unit)
  const [localTime, setLocalTime] = useState(owned?.schedule.localTime ?? '09:00')
  const [weekdays, setWeekdays] = useState<number[]>(owned?.schedule.scheduleKind === 'weekly' ? owned.schedule.weekdays ?? [] : [5])
  const [enabled, setEnabled] = useState(owned?.schedule.enabled ?? true)
  const [error, setError] = useState('')
  const submit = async () => {
    setError('')
    try {
      if (!name.trim() || !objective.trim()) throw new Error('请填写任务名称和执行要求。')
      let schedule: CompanionSchedule
      if (kind === 'once') {
        const unchanged = originalOnceAt != null && onceAt === localInputValue(originalOnceAt, timezone)
        const atMs = unchanged ? originalOnceAt : wallClockMs(onceAt, timezone)
        if (atMs <= Date.now()) throw new Error('一次性执行时间必须晚于当前时间。')
        schedule = { kind, atMs, timezone }
      } else if (kind === 'interval') {
        const amount = Number(intervalValue)
        const intervalSeconds = amount * Number(intervalUnit)
        if (!Number.isFinite(amount) || amount <= 0 || !Number.isInteger(intervalSeconds) || intervalSeconds < 60 || intervalSeconds > 2_592_000) throw new Error('间隔需在 1 分钟到 30 天之间。')
        schedule = { kind, intervalSeconds, timezone }
      } else {
        if (!/^\d{2}:\d{2}$/.test(localTime)) throw new Error('请选择明确的本地执行时间。')
        if (kind === 'weekly' && !weekdays.length) throw new Error('每周任务至少选择一个星期。')
        schedule = { kind, weekdays: kind === 'daily' ? ALL_WEEKDAYS : weekdays, localTime, timezone }
      }
      await onSave({ name: name.trim(), objective: objective.trim(), schedule, enabled })
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }
  return <form className="fox-contact-task-editor" onSubmit={event => { event.preventDefault(); void submit() }}>
    <h3>{owned ? '编辑任务' : '添加任务'}</h3>
    <label>任务名称<input aria-label="任务名称" value={name} onChange={event => setName(event.target.value)} required /></label>
    <label>执行要求<textarea aria-label="执行要求" value={objective} onChange={event => setObjective(event.target.value)} required /></label>
    <label>执行频率<select aria-label="执行频率" value={kind} onChange={event => setKind(event.target.value as ScheduleKind)}>
      <option value="once">一次性</option><option value="interval">间隔</option><option value="daily">每天</option><option value="weekly">每周</option>
    </select></label>
    {kind === 'once' && <label>执行时间<input aria-label="执行时间" type="datetime-local" value={onceAt} onChange={event => setOnceAt(event.target.value)} required /></label>}
    {kind === 'interval' && <div className="fox-contact-task-interval"><label>每隔<input aria-label="间隔数值" type="number" min="1" step="1" value={intervalValue} onChange={event => setIntervalValue(event.target.value)} /></label><label>单位<select aria-label="间隔单位" value={intervalUnit} onChange={event => setIntervalUnit(event.target.value as typeof intervalUnit)}><option value="1">秒</option><option value="60">分钟</option><option value="3600">小时</option><option value="86400">天</option></select></label></div>}
    {(kind === 'daily' || kind === 'weekly') && <label>本地时间<input aria-label="本地时间" type="time" value={localTime} onChange={event => setLocalTime(event.target.value)} required /></label>}
    {kind === 'weekly' && <fieldset><legend>星期</legend><div className="fox-contact-task-weekdays">{WEEKDAYS.map(([label, day]) => <label key={day}><input type="checkbox" checked={weekdays.includes(day)} onChange={event => setWeekdays(current => event.target.checked ? [...current, day].sort() : current.filter(value => value !== day))} />{label}</label>)}</div></fieldset>}
    <p className="fox-contact-task-meta">时区：{timezone}{owned ? '（保持不变）' : '（设备时区）'}</p>
    <label className="fox-contact-task-enabled"><input type="checkbox" checked={enabled} onChange={event => setEnabled(event.target.checked)} />启用任务</label>
    {error && <p role="alert">{error}</p>}
    <div className="fox-contact-task-actions"><Button type="submit" size="sm" disabled={busy}>保存</Button><Button type="button" size="sm" variant="ghost" disabled={busy} onClick={onCancel}>取消</Button></div>
  </form>
}
