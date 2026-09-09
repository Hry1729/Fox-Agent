import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { parseQueryArguments, reconciliationReady } from '@/features/conversations/model/reconciliation'
import type { ReconciliationDecision, ReconciliationOptions, ReconciliationView } from '@/features/conversations/model/reconciliation'
import './kernel-reconciliation.css'

const toolLabel: Record<string, string> = { write_file: '文件写入', edit_file: '文件编辑', read: '文件读取', call_mcp_tool: '连接器操作', initial_model: '模型请求', deliver_tool_batch: '模型续答', context_compaction: '上下文压缩' }
const decisionLabel = { executed: '已执行', not_executed: '未执行', unresolved: '暂不恢复' }

export function KernelReconciliationPanel({ conversationId, runId, onResumed }: {
  conversationId: string; runId: string; onResumed: () => Promise<void>
}) {
  const [view, setView] = useState<ReconciliationView | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [recoveryMode, setRecoveryMode] = useState<'read_only' | 'reapprove'>('read_only')
  const [options, setOptions] = useState<Record<string, ReconciliationOptions>>({})
  const [drafts, setDrafts] = useState<Record<string, { decision: ReconciliationDecision | ''; note: string; tool: string; args: string }>>({})
  useEffect(() => {
    let current = true
    setView(null); setOptions({}); setDrafts({}); setError(null); setRecoveryMode('read_only')
    void desktopClient.reconciliationLoad({ conversationId, runId }).then(value => { if (current) setView(value) })
      .catch(() => { if (current) setError('核对记录暂时无法读取，请刷新重试。') })
    return () => { current = false }
  }, [conversationId, runId])
  const request = (effectKey = '') => ({ conversationId, runId, expectedRevision: view?.revision ?? 0, effectKey })
  const unsaved = view?.items.some(item => drafts[item.effectKey] && (
    drafts[item.effectKey].decision !== (item.decision ?? '') || drafts[item.effectKey].note !== (item.note ?? '')
  )) ?? false
  async function act(action: () => Promise<void>) {
    if (busy) return
    setBusy(true); setError(null)
    try { await action() } catch (cause) { setError(cause instanceof Error ? cause.message : '操作未完成，请刷新后重试。') }
    finally { setBusy(false) }
  }
  if (!view && !error || view?.items.length === 0) return null
  return <section className="fox-reconciliation" aria-label="中断操作核对" aria-busy={busy}>
    <div className="fox-reconciliation-heading">
      <div><strong>有执行结果需要核对</strong><p>先核对中断操作，再选择如何继续。</p></div>
      <Button variant="outline" size="sm" onClick={() => setExpanded(value => !value)} aria-expanded={expanded} aria-controls={`reconciliation-${runId}`}>{expanded ? '收起' : '核对与恢复'}</Button>
    </div>
    {error && <p role="alert" className="fox-reconciliation-error">{error} <Button variant="ghost" size="sm" disabled={busy} onClick={() => void act(async () => { setView(await desktopClient.reconciliationLoad({ conversationId, runId })) })}>刷新</Button></p>}
    {expanded && view && <div id={`reconciliation-${runId}`} className="fox-reconciliation-body">
      <p>查询证据与人工确认会分别记录。恢复将创建关联的新任务，保留原任务记录。</p>
      {view.items.map((item, index) => {
        const draft = drafts[item.effectKey] ?? { decision: item.decision ?? '', note: item.note ?? '', tool: '', args: '{}' }
        const option = options[item.effectKey]
        const fieldId = `reconciliation-${runId}-${index}`
        const update = (value: Partial<typeof draft>) => setDrafts(current => ({ ...current, [item.effectKey]: { ...draft, ...value } }))
        return <div className="fox-reconciliation-item" key={item.effectKey}>
          <h3>{toolLabel[item.tool] ?? item.tool}{item.decision && <span>人工确认：{decisionLabel[item.decision]}</span>}</h3>
          <details><summary>查看原操作内容</summary><pre>{JSON.stringify(item.input, null, 2)}</pre></details>
          <div className="fox-reconciliation-actions"><Button variant="outline" size="sm" disabled={busy || Boolean(view.recoveryRunId)} onClick={() => void act(async () => {
            const result = await desktopClient.reconciliationOptions(request(item.effectKey))
            setOptions(current => ({ ...current, [item.effectKey]: result }))
          })}>查看查询方式</Button></div>
          {option && <div className="fox-reconciliation-query">
            <p>{option.message}</p>
            {option.kind === 'connector' && option.tools.length === 0 && <p>连接器未提供符合要求的查询工具，请填写外部核对依据。</p>}
            {option.kind === 'connector' && option.tools.length > 0 && <>
              <label htmlFor={`${fieldId}-tool`}>只读查询工具</label><select id={`${fieldId}-tool`} value={draft.tool} disabled={busy} onChange={event => update({ tool: event.target.value })}><option value="">请选择查询工具</option>{option.tools.map(tool => <option key={tool.name} value={tool.name}>{tool.description || tool.name}（{tool.name}）</option>)}</select>
              <label htmlFor={`${fieldId}-args`}>查询条件（JSON）</label><textarea id={`${fieldId}-args`} rows={3} value={draft.args} disabled={busy} onChange={event => update({ args: event.target.value })} />
              {draft.tool && <details><summary>查看查询条件说明</summary><pre>{JSON.stringify(option.tools.find(tool => tool.name === draft.tool)?.inputSchema, null, 2)}</pre></details>}
            </>}
            {(option.kind === 'file' || option.kind === 'connector' && option.tools.length > 0) && <Button variant="outline" size="sm" disabled={busy || Boolean(view.recoveryRunId) || option.kind === 'connector' && !draft.tool} onClick={() => void act(async () => {
              const next = await desktopClient.reconciliationQuery({ ...request(item.effectKey), queryTool: draft.tool || undefined, arguments: option.kind === 'connector' ? parseQueryArguments(draft.args) : undefined })
              setView(next); update({ decision: '', note: '' })
            })}>{busy ? '正在查询…' : option.kind === 'file' ? '查询原目标文件' : '执行只读查询'}</Button>}
          </div>}
          {item.evidence && <div className="fox-reconciliation-evidence"><strong>查询证据</strong>
            {item.evidence.source === 'independent_file_read' && <p>{item.evidence.status === 'unavailable' ? '文件暂时无法核验。' : `已独立读取 ${item.evidence.bytes} 字节。${item.evidence.matchesSubmittedContent === true ? '内容与原提交一致。' : item.evidence.matchesSubmittedContent === false ? '内容与原提交不同。' : ''}`}</p>}
            <p>{String(item.evidence.meaning ?? '')}</p><details><summary>查看详细查询记录</summary><pre>{JSON.stringify(item.evidence, null, 2)}</pre></details></div>}
          <label htmlFor={`${fieldId}-decision`}>人工核对结论</label>
          <select id={`${fieldId}-decision`} value={draft.decision} disabled={busy || Boolean(view.recoveryRunId)} onChange={event => update({ decision: event.target.value as ReconciliationDecision })}><option value="">请选择核对结论</option><option value="executed">已执行：已在原系统确认</option><option value="not_executed">未执行：已有明确依据</option><option value="unresolved">仍不确定，暂不恢复</option></select>
          <label htmlFor={`${fieldId}-note`}>核对依据</label><textarea id={`${fieldId}-note`} rows={3} maxLength={4096} value={draft.note} disabled={busy || Boolean(view.recoveryRunId)} placeholder="记录回执编号、查询结果或你在外部系统核对的情况。" onChange={event => update({ note: event.target.value })} />
          <Button size="sm" variant="outline" disabled={busy || Boolean(view.recoveryRunId) || !draft.decision || !draft.note.trim()} onClick={() => void act(async () => {
            setView(await desktopClient.reconciliationConfirm({ ...request(item.effectKey), decision: draft.decision as ReconciliationDecision, note: draft.note }))
          })}>保存人工确认</Button>
        </div>
      })}
      <div className="fox-reconciliation-footer">
        {!view.recoveryRunId && <fieldset disabled={busy}><legend>继续方式</legend>
          <label><input type="radio" name={`recovery-mode-${runId}`} checked={recoveryMode === 'read_only'} onChange={() => setRecoveryMode('read_only')} />只读核验：核对当前结果并列出后续步骤</label>
          <label><input type="radio" name={`recovery-mode-${runId}`} checked={recoveryMode === 'reapprove'} onChange={() => setRecoveryMode('reapprove')} />重新审批后继续：后续写入和外部操作重新申请批准</label>
        </fieldset>}
        <p>{view.recoveryRunId ? '已创建关联的恢复任务。' : unsaved ? '核对结论有未保存的修改，请先保存。' : reconciliationReady(view) ? recoveryMode === 'reapprove' ? '确认已保存。先核验当前状态，后续操作重新审批；旧授权不会沿用。' : '确认已保存。下一步只读核验，不重复执行原操作。' : '请逐项保存明确结论；仍不确定时保留记录，暂不恢复。'}</p>
        <Button size="sm" disabled={busy || unsaved || !reconciliationReady(view)} onClick={() => void act(async () => {
          await desktopClient.reconciliationResume({ ...request(), recoveryMode }); await onResumed()
        })}>{busy ? '处理中…' : recoveryMode === 'reapprove' ? '创建待审批的恢复任务' : '继续只读核验'}</Button>
      </div>
    </div>}
  </section>
}
