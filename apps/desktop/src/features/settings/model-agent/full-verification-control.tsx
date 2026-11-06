import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { nativePermissionRows, type NativePermissionView } from '@/features/conversations/model/agent-native-permission'
import { FULL_ACCESS_RISK_POINTS } from '@/features/conversations/model/agent-backend-onboarding'
import { AGENT_NAMES } from './agent-config-copy'
import type { ExternalAgentId } from './agent-config-types'

export function FullVerificationControl({ backend, busy, onStart }: {
  backend: ExternalAgentId
  busy: boolean
  onStart: (permissionMode: string, confirmFullAccess: boolean) => Promise<void>
}) {
  const [view, setView] = useState<NativePermissionView | null>(null)
  const [mode, setMode] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const rows = nativePermissionRows(view)
  const selected = rows.find(row => row.value === mode && !row.disabled)
  async function open() {
    setLoading(true)
    setError('')
    try {
      const next = await desktopClient.agentBackendNativePermissions({ backend })
      const current = nativePermissionRows(next).find(row => row.value === next.current && !row.disabled)
      setMode(current?.value ?? next.catalog.defaultMode)
      setView(next)
    } catch {
      setError('无法读取验证权限，请重试。')
    } finally {
      setLoading(false)
    }
  }
  async function start() {
    if (!selected) return
    if (selected.fullAccess && !await confirmDialog({
      title: '使用完全访问权限验证？',
      description: FULL_ACCESS_RISK_POINTS.join(''),
      confirmLabel: '使用完全访问权限',
      destructive: true,
    })) return
    setView(null)
    await onStart(selected.value, selected.fullAccess)
  }
  return <div className="fox-model-actions fox-model-section">
    {view ? <>
      <label>本次验证权限 <select value={mode} disabled={busy} onChange={e => setMode(e.target.value)}>
        {rows.filter(row => view.catalog.options.find(option => option.id === row.value)?.listed !== false).map(row =>
          <option key={row.value} value={row.value} disabled={row.disabled} title={row.reason ?? row.title}>{row.label}</option>)}
      </select></label>
      <Button size="sm" disabled={busy || !selected} onClick={() => void start()}>开始验证</Button>
      <Button variant="ghost" size="sm" onClick={() => setView(null)}>取消</Button>
    </> : <Button variant="ghost" size="sm" disabled={loading || busy} onClick={() => void open()} aria-label={AGENT_NAMES[backend] + ' 完整验证：Agent 输出和工具调用'}>完整验证：Agent 输出和工具调用</Button>}
    {error && <span role="alert">{error}</span>}
  </div>
}
