// Settings batch, items 4 / 5: 设置 → 助手设置 →「外部 Agent」.
//
// *「新会话使用的 Agent」: Fox / Codex / pi / DeepSeek (default Fox). Every
//   NEW conversation uses it; existing conversations keep the backend they
//   were created with. A backend the administrator forced off
//   (FOX_EXTERNAL_BACKENDS), or Codex when 模型服务's provider has no
//   Responses API, is greyed with the reason.
// *「新外部会话使用完全访问」: off by default; read by the Host when it
//   creates an external conversation (grant + audit as before); greyed under
//   FORBIDDEN. Revoking mid-session stays on the bar above the input.
//
// Saved by the Host (`agent_backend_preferences_save`, `app_settings`); a
// change is broadcast so the open draft re-probes its login at once.
import { useCallback, useEffect, useState } from 'react'
import { Switch } from '@/components/ui/switch'
import { Card } from '@/components/ui/card'
import {
  AGENT_BACKEND_PREFERENCES_EVENT,
  agentBackendForcedOffNotice,
  agentBackendSettingRows,
  fullAccessSettingState,
  isAgentBackendId,
  type AgentBackendId,
  type AgentBackendPreferences,
  type CodexResponsesProbe,
} from '@/features/conversations/model/agent-backend'
import { FULL_ACCESS_RISK_POINTS } from '@/features/conversations/model/agent-backend-onboarding'

export interface AgentBackendSettingsClient {
  agentBackendPreferences: () => Promise<AgentBackendPreferences>
  saveAgentBackendPreferences: (change: { defaultBackend?: AgentBackendId; newExternalFullAccess?: boolean }) => Promise<AgentBackendPreferences>
  probeCodexResponses: () => Promise<CodexResponsesProbe | null>
}

export function dispatchAgentBackendPreferencesChanged(preferences: AgentBackendPreferences) {
  if (typeof window === 'undefined') return
  window.dispatchEvent(new CustomEvent(AGENT_BACKEND_PREFERENCES_EVENT, { detail: preferences }))
}

export function AgentBackendSettingsSection({ client }: { client: AgentBackendSettingsClient }) {
  const [preferences, setPreferences] = useState<AgentBackendPreferences | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    let cancelled = false
    client.agentBackendPreferences()
      .then((value) => { if (!cancelled) setPreferences(value) })
      .catch((cause) => { if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause)) })
    return () => { cancelled = true }
  }, [client])

  const save = useCallback(async (change: { defaultBackend?: AgentBackendId; newExternalFullAccess?: boolean }) => {
    setSaving(true)
    setError(null)
    try {
      const saved = await client.saveAgentBackendPreferences(change)
      setPreferences(saved)
      dispatchAgentBackendPreferencesChanged(saved)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }, [client])

  const rows = agentBackendSettingRows(preferences)
  const forcedOff = agentBackendForcedOffNotice(preferences)
  const fullAccess = fullAccessSettingState(preferences)
  const selected = preferences?.defaultBackend ?? 'fox'
  const effective = preferences?.effectiveBackend ?? 'fox'
  const unavailableReasons = rows.filter((row) => row.disabled && row.reason)
  return (
    <Card className="fox-settings-section fox-agent-backend-settings" aria-label="外部 Agent 设置">
      <div className="fox-settings-section-head"><div><h2>外部 Agent</h2><p>新会话由哪个 Agent 执行。只影响之后新建的会话，已有会话保持原来的后端。</p></div></div>
      <div className="fox-setting-row fox-setting-row-bordered">
        <span><b>新会话使用的 Agent</b><small>默认 Fox。外部 Agent 自动使用当前供应商；缺少地址时会在后端栏提示补充。已有登录优先。</small></span>
        <select
          aria-label="新会话使用的 Agent"
          value={selected}
          disabled={!preferences || saving}
          onChange={(event) => {
            const next = event.target.value
            if (isAgentBackendId(next) && !rows.find((row) => row.value === next)?.disabled) void save({ defaultBackend: next })
          }}
        >
          {rows.map((row) => (
            <option key={row.value} value={row.value} disabled={row.disabled} title={row.reason ?? undefined}>
              {row.label}{row.disabled ? '（不可用）' : ''}
            </option>
          ))}
        </select>
      </div>
      {forcedOff && <p className="fox-agent-backend-settings-note" role="note">{forcedOff}</p>}
      {!forcedOff && unavailableReasons.length > 0 && (
        <ul className="fox-agent-backend-settings-note" aria-label="不可用的 Agent">
          {unavailableReasons.map((row) => <li key={row.value}>{row.label}：{row.reason}</li>)}
        </ul>
      )}
      {preferences && selected !== effective && (
        <p className="fox-agent-backend-settings-note" role="note">已保存的选择当前不可用，新会话暂时使用 {rows.find((row) => row.value === effective)?.label ?? 'Fox'}。</p>
      )}
      <div className="fox-setting-row fox-setting-row-bordered">
        <span>
          <b>新外部会话使用完全访问</b>
          <small>默认关闭（沙箱）。开启后新建的外部会话不套沙箱：{FULL_ACCESS_RISK_POINTS.join('')}会话中途只能在输入框上方撤销。</small>
        </span>
        <Switch
          aria-label="新外部会话使用完全访问"
          checked={fullAccess.checked}
          disabled={fullAccess.disabled || saving}
          onCheckedChange={(value) => void save({ newExternalFullAccess: value })}
        />
      </div>
      {fullAccess.reason && preferences?.fullAccessForbidden && <p className="fox-agent-backend-settings-note" role="note">{fullAccess.reason}</p>}
      {error && <p className="fox-agent-backend-settings-error" role="alert">{error}</p>}
    </Card>
  )
}
