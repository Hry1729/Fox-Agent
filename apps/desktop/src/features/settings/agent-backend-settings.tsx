// Settings batch, items 4 / 5: 设置 → Agent →「Agent 驱动」(rows in the
// unified settings layout).
//
// *「新会话使用的 Agent」is a plain statement since the single Pi mainline
//   (product decision 2026-10-27): only the built-in Fox route (model engine
//   Pi) can drive a new conversation, so there is no select to make. A saved
//   backend of an older release is reported as no longer applicable, and old
//   conversations keep their own binding (read-only).
// * Each external Agent's native default permission is no longer offered: it
//   only ever applied to a new conversation of a driver this build retired.
//   Fox's own permission settings live on this page (项目权限) and stay.
//
// The preferences are still read from the Host (`agent_backend_preferences_get`,
// `app_settings`); this page no longer writes one, because there is no choice to
// save. `dispatchAgentBackendPreferencesChanged` stays wired for a Host that
// ever offers more than one driver again.
import { useEffect, useState } from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { SettingsGroup, SettingsRow } from './settings-layout'
import {
  AGENT_BACKEND_PREFERENCES_EVENT,
  agentBackendForcedOffNotice,
  agentBackendSavedRetiredNotice,
  isAgentBackendHidden,
  isAgentBackendRetired,
  NEW_CONVERSATION_AGENT_BACKEND,
  NEW_CONVERSATION_BACKEND_STATEMENT,
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
 * (hidden and retired backends left out). Empty since the single Pi mainline:
 * an Agent's own "default permission" only applies to a NEW conversation of
 * that Agent, and no retired backend can have one. The Host command and
 * `AgentNativePermissionPicker` stay for the records already made, and
 * Fox's own permission settings are on this page (项目权限).
 */
export const NATIVE_PERMISSION_SETTING_BACKENDS: readonly { backend: AgentBackendId; label: string }[] = ([
  { backend: 'codex', label: 'Codex' },
  { backend: 'deepseek', label: 'DeepSeek' },
  { backend: 'pi', label: 'Pi' },
] as const).filter(({ backend }) => !isAgentBackendHidden(backend) && !isAgentBackendRetired(backend))

/**
 * 设置 → each Agent's default permission for its NEW conversations, in its
 * own names. Full access is left out of the default; it stays an explicit
 * per-conversation choice in the Agent's own picker.
 *
 * Renders nothing while `NATIVE_PERMISSION_SETTING_BACKENDS` is empty (every
 * external Agent is retired): kept so the Host command and the row layout stay
 * in place, and so re-enabling a driver is a one-line change.
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

  useEffect(() => {
    let cancelled = false
    client.agentBackendPreferences()
      .then((value) => { if (!cancelled) setPreferences(value) })
      .catch((cause) => { if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause)) })
    return () => { cancelled = true }
  }, [client])

  const forcedOff = agentBackendForcedOffNotice(preferences)
  const savedRetired = agentBackendSavedRetiredNotice(preferences)
  return (
    <SettingsGroup title="Agent 驱动" className="fox-agent-backend-settings" aside={<span>只影响之后新建的会话</span>}>
      {/* One supported driver: a select with one option is not a choice, so the
          row states what a new conversation uses instead of offering a pick. */}
      <SettingsRow title="新会话使用的 Agent" description="本版本只支持一个 Agent 驱动；会话创建后不能更换，已有会话保留创建时的绑定。">
        <span className="fox-settings-value" data-agent-backend={NEW_CONVERSATION_AGENT_BACKEND} aria-live="polite">{preferences ? NEW_CONVERSATION_BACKEND_STATEMENT : '正在读取…'}</span>
      </SettingsRow>
      {forcedOff && <p className="fox-settings-footnote fox-agent-backend-settings-note" role="note">{forcedOff}</p>}
      {savedRetired && <p className="fox-settings-footnote fox-agent-backend-settings-note" role="note" data-retired-default={preferences?.defaultBackend}>{savedRetired}</p>}
      <NativePermissionDefaults client={client} />
      {error && <p className="fox-settings-error fox-agent-backend-settings-error" role="alert">{error}</p>}
    </SettingsGroup>
  )
}
