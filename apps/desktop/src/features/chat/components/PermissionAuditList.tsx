import { toolRecordPresentation } from '../tool-record-presentation'
import { SideCard, SideSection } from '@/components/fox-ui/side-section'
import { useEffect, useState } from 'react'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { PermissionAuditRecord } from '@/features/conversations/model/types'
import { userFacingCause } from '@/features/chat/send-failure-notice'

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
      .catch(cause => { if (!cancelled) setError(userFacingCause(cause)) })
    return () => { cancelled = true }
  }, [runId])
  if (!runId) return null
  return <SideSection title="自动放行" count={entries.length}><SideCard className="fox-activity-events fox-permission-audit">
    {error && <p className="fox-activity-warning">{error}</p>}
    {!error && entries.length === 0 && <p className="fox-activity-empty">这次运行没有未经询问就执行的写入、命令或联网操作。</p>}
    {entries.map(entry => {
      const { action, Icon } = toolRecordPresentation({ toolName: entry.tool, input: {}, status: 'completed' })
      return <div className="fox-record-row" key={entry.id} title={entry.findings.join(', ')}>
        <Icon /><span className="fox-record-copy"><strong className={entry.risk === 'high' ? 'is-warning' : undefined}>{action}{entry.risk === 'high' ? ' · 高风险' : ''}</strong>
        <small>{[entry.summary, auditReasonLabel(entry), new Date(entry.createdAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })].filter(Boolean).join(' · ')}</small></span>
      </div>
    })}
  </SideCard></SideSection>
}
