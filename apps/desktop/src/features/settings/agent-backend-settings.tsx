// Settings batch, items 4 / 5: 设置 → Agent →「Agent 驱动」(rows in the
// unified settings layout).
//
// *「新会话使用的 Agent」(plan C1): Fox (built-in PiNative) / Pi / Codex /
//   DeepSeek (dsh), default Fox; every backend is selectable unless the
//   administrator forced it off (`FOX_EXTERNAL_BACKENDS`). A saved choice
//   that cannot be used now is reported, and a new conversation then uses Fox.
// * Each external Agent's default permission for its NEW conversations, as
//   one of Fox's four tiers mapped to that Agent's own mode (full access is
//   left out of the default: it stays an explicit, confirmed choice).
//
// Saved by the Host (`agent_backend_preferences_save`, `app_settings`); a
// change is broadcast so the open draft re-probes its login at once.
import { useCallback, useEffect, useState } from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
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
function NativePermissionDefaults({ client }: { client: AgentBackendSettingsClient }) {
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
      {NATIVE_PERMISSION_SETTING_BACKENDS.map(({ backend, label }) => {
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
  const savedRetired = agentBackendSavedRetiredNotice(preferences)
  const selected = agentBackendSettingValue(preferences)
  const effective = isAgentBackendId(preferences?.effectiveBackend) ? preferences.effectiveBackend : 'fox'
  const unavailableReasons = rows.filter((row) => row.disabled && row.reason)
  return (
    <SettingsGroup title="Agent 驱动" className="fox-agent-backend-settings" aside={<span>只影响之后新建的会话</span>}>
      <SettingsRow title="新会话使用的 Agent" description="默认 Fox（内置 PiNative）；也可选外部 Agent：Pi、Codex、DeepSeek（dsh）。会话创建后不能更换，已有会话保留创建时的绑定。">
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
        <ul className="fox-settings-footnote fox-agent-backend-settings-note" aria-label="不可用的 Agent">
          {unavailableReasons.map((row) => <li key={row.value}>{row.label}：{row.reason}</li>)}
        </ul>
      )}
      {savedRetired && <p className="fox-settings-footnote fox-agent-backend-settings-note" role="note" data-retired-default={preferences?.defaultBackend}>{savedRetired}</p>}
      {preferences && !savedRetired && selected !== effective && (
        <p className="fox-settings-footnote fox-agent-backend-settings-note" role="note">已保存的选择当前不可用，新会话暂时使用 {AGENT_BACKEND_LABELS[effective]}。</p>
      )}
      <NativePermissionDefaults client={client} />
      {error && <p className="fox-settings-error fox-agent-backend-settings-error" role="alert">{error}</p>}
    </SettingsGroup>
  )
}
