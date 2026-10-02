import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { ApiResponse } from '@/features/conversations/model/types'
import { Button } from '@/components/ui/button'
import { ContextBudgetPanel } from './ContextBudgetPanel'
import type { ContextBudgetDto } from './context-budget-presentation'
import { JobProgressCard } from '@/features/jobs/JobProgressCard'
import type { JobSnapshot } from '@/features/jobs/job-presentation'
import { readRunBudget, saveRunBudget, type RunBudgetSelection } from './run-budget'

interface Snapshot { runId: string | null; contextBudget: ContextBudgetDto | null; jobs: JobSnapshot[]; continuable: { runId: string; pauseReason: string; completedToolCalls: number }[] }
/** How a paused run reads to the user; the wording names what is still missing. */
function pauseLabel(pauseReason: string): string {
  switch (pauseReason) {
    case 'approval_expired': return '审批已过期'
    case 'context_limit': return '上下文需要调整'
    case 'model_failure': return '模型请求失败，已完成的工作可以继续'
    default: return '执行预算已用完'
  }
}
async function call<T>(command: string, request: unknown): Promise<T> {
  const result = await invoke<ApiResponse<T>>(command, { request })
  if (!result.ok) throw new Error(result.error.message)
  return result.data
}
export function RunReliabilityPanel({ conversationId, active }: { conversationId?: string; active: boolean }) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null)
  const [budget, setBudget] = useState<RunBudgetSelection>({ tier: 'standard' })
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [page, setPage] = useState<{ jobId: string; content: string; nextOffset: number | null; complete: boolean } | null>(null)
  useEffect(() => {
    setSnapshot(null); setPage(null); setError(null)
    if (!conversationId || !desktopRuntimeAvailable) return
    setBudget(readRunBudget(conversationId))
    let alive = true
    const load = async () => {
      try { const data = await call<Snapshot>('kernel_reliability_snapshot', { conversationId }); if (alive) { setSnapshot(data); setError(null) } }
      catch (cause) { if (alive) setError(String(cause)) }
    }
    void load()
    const timer = window.setInterval(() => void load(), active ? 1500 : 5000)
    return () => { alive = false; window.clearInterval(timer) }
  }, [conversationId, active])
  if (!conversationId || !desktopRuntimeAvailable) return null
  const select = (next: RunBudgetSelection) => { saveRunBudget(conversationId, next); setBudget(next) }
  const jobAction = async (job: JobSnapshot, tool: string, input: unknown) => {
    const value = await call<{ content: { text: string }[] }>('kernel_job_execute', { conversationId, runId: job.runId, tool, input })
    return JSON.parse(value.content[0].text)
  }
  const readPage = async (job: JobSnapshot, offset = 0) => {
    try { setPage(await jobAction(job, 'compute_job_result', { jobId: job.jobId, offset, limit: 16384 })); setError(null) }
    catch (cause) { setError(String(cause)) }
  }
  const continueRun = async (sourceRunId: string) => {
    setBusy(true); setError(null)
    try {
      const started = await call('kernel_run_continue', { conversationId, sourceRunId, ...budget })
      window.dispatchEvent(new CustomEvent('fox:continued-run', { detail: { conversationId, started } }))
      setSnapshot(await call<Snapshot>('kernel_reliability_snapshot', { conversationId }))
    } catch (cause) { setError(String(cause)) } finally { setBusy(false) }
  }
  return <details className="mx-3 my-2 rounded border p-2 text-sm">
    <summary>任务预算与进度{snapshot?.jobs.some(j => j.state === 'running') ? ' · 有后台作业' : ''}</summary>
    <div className="mt-2 space-y-3">
      <label className="flex items-center gap-2">下次任务预算
        <select aria-label="下次任务预算" disabled={active || busy} value={budget.tier} onChange={e => select(e.target.value === 'custom' ? { tier: 'custom', customExecutionMs: 3600000 } : { tier: e.target.value as RunBudgetSelection['tier'] })}>
          <option value="short">短任务 · 10 分钟</option><option value="standard">标准 · 30 分钟</option><option value="long">长任务 · 2 小时</option><option value="custom">自定义</option>
        </select>
        {budget.tier === 'custom' && <input aria-label="预算分钟数" type="number" min="1" max="1440" disabled={active || busy} value={(budget.customExecutionMs ?? 3600000) / 60000} onChange={e => { const n = Number(e.target.value); if (Number.isInteger(n) && n > 0 && n <= 1440) select({ tier: 'custom', customExecutionMs: n * 60000 }) }} />}
      </label>
      <p className="text-xs text-muted-foreground">审批等待不计入执行预算；当前任务的预算保持冻结。</p>
      {snapshot?.contextBudget && <ContextBudgetPanel budget={snapshot.contextBudget} />}
      {snapshot?.continuable.map(run => <div key={run.runId} className="flex items-center justify-between gap-2">
        <span>{pauseLabel(run.pauseReason)} · 已完成 {run.completedToolCalls} 次工具调用</span>
        <Button size="sm" disabled={active || busy} onClick={() => void continueRun(run.runId)}>保留进度继续</Button>
      </div>)}
      {snapshot?.jobs.map(job => <div key={job.jobId}>
        <JobProgressCard snapshot={job} onCancel={async id => { await jobAction(job, 'compute_job_cancel', { jobId: id }) }} onViewResult={() => void readPage(job)} />
        {job.resumable && <Button size="sm" disabled={busy || !active || job.runId !== snapshot.runId} onClick={() => { void jobAction(job, 'compute_job_start', { jobId: job.jobId }).catch(cause => setError(String(cause))) }}>重新检查权限并恢复作业</Button>}
      </div>)}
      {page && <div><pre className="max-h-72 overflow-auto whitespace-pre-wrap">{page.content}</pre>{page.nextOffset !== null && <Button size="sm" onClick={() => { const job = snapshot?.jobs.find(j => j.jobId === page.jobId); if (job) void readPage(job, page.nextOffset!) }}>下一页</Button>}</div>}
      {error && <p role="alert" className="text-destructive">{error}</p>}
    </div>
  </details>
}
