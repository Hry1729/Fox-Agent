import { useCallback, useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { isAgentEnabled, useEnabledExternalAgents } from '@/features/conversations/model/external-agent-enablement'
import { SettingsGroup, SettingsRow } from './settings-layout'
import {
  AGENT_BACKEND_PREFERENCES_EVENT,
  agentBackendForcedOffNotice,
  agentBackendSavedRetiredNotice,
  agentBackendSettingRows,
  agentBackendSettingValue,
  AGENT_BACKEND_LABELS,
  isAgentBackendHidden,
  isAgentBackendId,
  isAgentBackendRetired,
  type AgentBackendId,
  type AgentBackendPreferences,
  type CodexResponsesProbe,
} from '@/features/conversations/model/agent-backend'
import {
  nativePermissionRows,
  type NativePermissionRequest,
  type NativePermissionView,
} from '@/features/conversations/model/agent-native-permission'

export interface AgentBackendSettingsClient {
  agentBackendPreferences: () => Promise<AgentBackendPreferences>
  saveAgentBackendPreferences: (change: { defaultBackend?: AgentBackendId; newExternalFullAccess?: boolean }) => Promise<AgentBackendPreferences>
  probeCodexResponses: () => Promise<CodexResponsesProbe | null>
  /** Per-agent native permission list; absent = no「默认权限」rows. */
  agentBackendNativePermissions?: (request: NativePermissionRequest) => Promise<NativePermissionView>
  agentBackendNativePermissionSet?: (request: NativePermissionRequest & { mode: string }) => Promise<NativePermissionView>

}

/**
 * The external Agents that have a native permission list, in settings order
 * (hidden and retired backends left out).
 */
export const NATIVE_PERMISSION_SETTING_BACKENDS: readonly { backend: AgentBackendId; label: string }[] = ([
  { backend: 'codex', label: 'Codex' },
  { backend: 'deepseek', label: 'DeepSeek' },
  { backend: 'pi', label: 'Pi' },
] as const).filter(({ backend }) => !isAgentBackendHidden(backend) && !isAgentBackendRetired(backend))

/**
 * 设置 → each Agent's default permission for its NEW conversations, as Fox's
 * tiers mapped to the Agent's own mode. Full access is left out of the
 * default; it stays an explicit per-conversation choice in the Agent's own
 * picker.
 */
function NativePermissionDefaults({ client, enabled }: { client: AgentBackendSettingsClient; enabled: readonly string[] }) {
  const list = client.agentBackendNativePermissions
  const set = client.agentBackendNativePermissionSet
  const [views, setViews] = useState<Partial<Record<AgentBackendId, NativePermissionView>>>({})
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!list) return
    let cancelled = false
    for (const { backend } of NATIVE_PERMISSION_SETTING_BACKENDS) {
      list({ backend })
        .then((view) => { if (!cancelled) setViews((current) => ({ ...current, [backend]: view })) })
        .catch(() => {})
    }
    return () => { cancelled = true }
  }, [list])
  if (!list || !set) return null
  return (
    <>
      {NATIVE_PERMISSION_SETTING_BACKENDS.filter(({ backend }) => isAgentEnabled(backend, enabled)).map(({ backend, label }) => {
        const view = views[backend]
        const rows = nativePermissionRows(view, { restrictedOnly: true })
        if (!view || rows.length === 0) return null
        const wanted = view.restrictedMode ?? view.current
        const current = rows.some((row) => row.value === wanted) ? wanted : view.catalog.defaultMode
        const greyed = rows.filter((row) => row.disabled && row.reason)
        return (
          <SettingsRow key={backend} className="fox-agent-native-default" title={`${label} 默认权限`} description={`${view.catalog.switchNote}${greyed.map((row) => `；${row.label}：${row.reason}`).join('')}`}>
            <Select
              value={current}
              onValueChange={(mode) => {
                if (rows.find((row) => row.value === mode)?.disabled) return
                setError(null)
                set({ backend, mode, defaultOnly: true })
                  .then((next) => setViews((items) => ({ ...items, [backend]: next })))
                  .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
              }}
            >
              <SelectTrigger className="fox-settings-agent-select" data-native-default={backend} aria-label={`${label} 默认权限`}><SelectValue /></SelectTrigger>
              <SelectContent>
                {rows.map((row) => (
                  <SelectItem key={row.value} value={row.value} data-value={row.value} disabled={row.disabled} title={row.title}>
                    {row.label}{row.disabled ? '（不可用）' : ''}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </SettingsRow>
        )
      })}
      {error && <p className="fox-settings-error fox-agent-backend-settings-error" role="alert">{error}</p>}
    </>
  )
}

/** Broadcast a preferences change (the open draft re-probes on it). */
export function dispatchAgentBackendPreferencesChanged(preferences: AgentBackendPreferences) {
  if (typeof window === 'undefined') return
  window.dispatchEvent(new CustomEvent(AGENT_BACKEND_PREFERENCES_EVENT, { detail: preferences }))
}

export function AgentBackendSettingsSection({ client, onOpenModelSettings }: { client: AgentBackendSettingsClient; onOpenModelSettings?: () => void }) {
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

  const enabled = useEnabledExternalAgents()
  const rows = agentBackendSettingRows(preferences, enabled)
  const forcedOff = agentBackendForcedOffNotice(preferences)
  const savedRetired = agentBackendSavedRetiredNotice(preferences)
  const selected = agentBackendSettingValue(preferences, enabled)
  const effective = isAgentBackendId(preferences?.effectiveBackend) ? preferences.effectiveBackend : 'fox'
  const unavailableReasons = rows.filter((row) => row.disabled && row.reason)
  return (
    <SettingsGroup title="Agent 驱动" className="fox-agent-backend-settings" aside={<span>模型服务、登录配置和启用开关在「模型与 Agent」中管理</span>}>
      {onOpenModelSettings && <SettingsRow title="模型与 Agent" description="选择服务、导入登录配置或验证 Agent。"><Button variant="ghost" onClick={onOpenModelSettings}>前往配置</Button></SettingsRow>}
      <SettingsRow title="新会话使用的 Agent" description="默认 Fox；启用的外部 Agent 也可以选。会话创建后不能更换 Agent。">
        <Select
          value={selected}
          disabled={!preferences || saving}
          onValueChange={(next) => {
            if (isAgentBackendId(next) && !rows.find((row) => row.value === next)?.disabled) void save({ defaultBackend: next })
          }}
        >
          <SelectTrigger className="fox-settings-agent-select" aria-label="新会话使用的 Agent"><SelectValue /></SelectTrigger>
          <SelectContent>
            {rows.map((row) => (
              <SelectItem key={row.value} value={row.value} data-value={row.value} disabled={row.disabled} title={row.reason ?? undefined}>
                {row.label}{row.disabled ? '（不可用）' : ''}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </SettingsRow>
      {forcedOff && <p className="fox-settings-footnote fox-agent-backend-settings-note" role="note">{forcedOff}</p>}
      {!forcedOff && unavailableReasons.length > 0 && (
        <div className="fox-settings-footnote fox-agent-backend-settings-note" role="note" aria-label="不可用的 Agent">
          {unavailableReasons.map((row) => <p key={row.value}>{row.label}：{row.reason}</p>)}
        </div>
      )}
      {savedRetired && <p className="fox-settings-footnote fox-agent-backend-settings-note" role="note" data-retired-default={preferences?.defaultBackend}>{savedRetired}</p>}
      {preferences && !savedRetired && selected !== effective && isAgentEnabled(effective, enabled) && (
        <p className="fox-settings-footnote fox-agent-backend-settings-note" role="note">已保存的选择当前不可用，新会话暂时使用 {AGENT_BACKEND_LABELS[effective]}。</p>
      )}
      <NativePermissionDefaults client={client} enabled={enabled} />
      {error && <p className="fox-settings-error fox-agent-backend-settings-error" role="alert">{error}</p>}
    </SettingsGroup>
  )
}
