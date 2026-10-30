// A1: what the Host allowed in a Run without asking (mode, matched rules, risk).
import { useEffect, useState } from 'react'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { PermissionAuditRecord } from '@/features/conversations/model/types'

const MODE_LABELS: Record<string, string> = { read_only: '只读', ask: '询问', auto: '自动', full_access: '完全访问' }
const REASON_LABELS: Record<string, string> = { mode: '当前模式自动放行', full_access: '完全访问', rule: '命中项目规则', read: '读取' }

export function auditReasonLabel(entry: Pick<PermissionAuditRecord, 'mode' | 'reason' | 'ruleIds'>) {
  const mode = MODE_LABELS[entry.mode] ?? entry.mode
  const reason = REASON_LABELS[entry.reason] ?? entry.reason
  return entry.reason === 'rule' && entry.ruleIds.length ? `${mode} · ${reason}（${entry.ruleIds.length} 条）` : `${mode} · ${reason}`
}

export function PermissionAuditList({ runId }: { runId: string | null | undefined }) {
  const [entries, setEntries] = useState<PermissionAuditRecord[]>([])
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let cancelled = false
    setEntries([])
    setError(null)
    if (!runId) return
    desktopClient.permissionAuditList({ runId, limit: 200 })
      .then(rows => { if (!cancelled) setEntries(rows) })
      .catch(cause => { if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause)) })
    return () => { cancelled = true }
  }, [runId])
  if (!runId) return null
  return <section className="fox-activity-events fox-permission-audit">
    <div className="fox-activity-section-head"><strong>自动放行记录</strong><small>{entries.length} 条</small></div>
    {error && <p className="fox-activity-warning">{error}</p>}
    {!error && entries.length === 0 && <p className="fox-activity-empty">这次运行没有未经询问就执行的写入、命令或联网操作。</p>}
    {entries.map(entry => <p key={entry.id} title={entry.findings.join(', ')}>
      <span>{entry.tool}{entry.risk === 'high' ? ' · 高风险' : ''}</span>
      <small>{auditReasonLabel(entry)} · {new Date(entry.createdAt).toLocaleTimeString()}</small>
      {entry.summary && <code>{entry.summary}</code>}
    </p>)}
  </section>
}
