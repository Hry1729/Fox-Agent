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
import { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import type { AgentModelConnection, AgentModelConnectionRequest } from '@/features/chat/components/AgentModelConnectionCard'
import type { AgentBackendTarget, CredentialProbe, ImportItem, ImportReport } from '@/features/conversations/model/agent-backend-onboarding'
import { EXTERNAL_AGENT_IDS, isAgentEnabled, setExternalAgentEnabled, useEnabledExternalAgents } from '@/features/conversations/model/external-agent-enablement'
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
  /** UI 1103: the existing login probe, read for each external Agent's status. */
  agentBackendCredentials?: (target: AgentBackendTarget) => Promise<CredentialProbe>
  agentBackendModelConnection?: (request: AgentModelConnectionRequest) => Promise<AgentModelConnection>
  /** Pi's「配置来源」: Fox 模型服务, or the native ~/.pi imported for every project. */
  agentBackendChooseFoxSource?: (target: AgentBackendTarget, remember: boolean) => Promise<unknown>
  agentBackendImportCandidates?: (target: AgentBackendTarget) => Promise<ImportItem[]>
  agentBackendImportCredentials?: (target: AgentBackendTarget, selected: string[], allProjects: boolean, overwrite?: boolean, reuseForNewProjects?: boolean) => Promise<ImportReport>
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

/** 设置 → Agent: what each external Agent's row says about it (UI 1103). */
export type ExternalAgentState = 'checking' | 'detected' | 'not_installed' | 'not_configured'

export const EXTERNAL_AGENT_STATE_LABELS: Record<ExternalAgentState, string> = {
  checking: '检测中',
  detected: '已检测到',
  not_installed: '未安装',
  not_configured: '未配置',
}

export const EXTERNAL_AGENT_SETTING_LABELS: Record<string, string> = { pi: 'Pi', codex: 'Codex', deepseek: 'DeepSeek（dsh）' }

/**
 * The state from the Host's existing probes: `installed` of the login probe
 * (the launch's own check), its `state`, and the model connection.
 */
export function externalAgentState(probe: CredentialProbe | null | undefined, connection: AgentModelConnection | null | undefined, failed = false): ExternalAgentState {
  if (failed) return 'not_configured'
  if (!probe) return 'checking'
  if (probe.installed === false) return 'not_installed'
  const connectionReady = !connection || connection.status === 'ready' || connection.status === 'existing_login'
  if (probe.state === 'ready' && !probe.source?.choiceRequired && connectionReady) return 'detected'
  return 'not_configured'
}

interface ExternalAgentProbe { probe: CredentialProbe | null; connection: AgentModelConnection | null; failed: boolean }

/**
 * 13.37: one Agent's status, in the Host's own order. The model connection
 * comes FIRST: it tests the address when needed and writes the Fox login
 * into the Agent's private HOME (`prepare_model_login`); the login probe read
 * before that step reports the HOME as not configured. Reading both at once
 * (the 1103 version) showed 「未配置」 until the next visit of the page.
 */
export async function probeExternalAgent(client: Pick<AgentBackendSettingsClient, 'agentBackendCredentials' | 'agentBackendModelConnection'>, backend: AgentBackendId): Promise<ExternalAgentProbe> {
  const credentials = client.agentBackendCredentials
  if (!credentials) return { probe: null, connection: null, failed: true }
  const target: AgentBackendTarget = { backend, fullAccess: false }
  const connection = client.agentBackendModelConnection ? await client.agentBackendModelConnection({ target }).catch(() => null) : null
  try {
    return { probe: await credentials(target), connection, failed: false }
  } catch {
    return { probe: null, connection, failed: true }
  }
}

/** The external Agents with an enable switch, their status and Pi's configuration source. */
function ExternalAgentRows({ client, enabled, onOpenModelSettings }: { client: AgentBackendSettingsClient; enabled: readonly string[]; onOpenModelSettings?: () => void }) {
  const [probes, setProbes] = useState<Partial<Record<string, ExternalAgentProbe>>>({})
  const [checking, setChecking] = useState<Partial<Record<string, boolean>>>({})
  const [sourceBusy, setSourceBusy] = useState(false)
  const [sourceError, setSourceError] = useState<string | null>(null)
  // Only the newest probe of each Agent may write its row.
  const generation = useRef<Record<string, number>>({})

  const inFlight = useRef<Record<string, boolean>>({})

  /** `quiet`: a focus / visibility refresh, skipped for an Agent already being checked. */
  const recheck = useCallback((backends: readonly AgentBackendId[], quiet = false) => {
    if (!client.agentBackendCredentials) return
    for (const backend of backends) {
      if (quiet && inFlight.current[backend]) continue
      inFlight.current[backend] = true
      const id = (generation.current[backend] ?? 0) + 1
      generation.current[backend] = id
      setChecking((current) => ({ ...current, [backend]: true }))
      void probeExternalAgent(client, backend).then((result) => {
        if (generation.current[backend] !== id) return
        inFlight.current[backend] = false
        setProbes((current) => ({ ...current, [backend]: result }))
        setChecking((current) => ({ ...current, [backend]: false }))
      })
    }
  }, [client])

  // Every row on first show; then again whenever 模型服务 changes, and when
  // the window or the page comes back into view (the user configured the
  // Agent or the model service somewhere else in the meantime).
  useEffect(() => {
    recheck(EXTERNAL_AGENT_IDS)
    const all = () => recheck(EXTERNAL_AGENT_IDS)
    const regained = () => recheck(EXTERNAL_AGENT_IDS, true)
    const visible = () => { if (document.visibilityState === 'visible') regained() }
    window.addEventListener('fox:model-service-changed', all)
    window.addEventListener('focus', regained)
    document.addEventListener('visibilitychange', visible)
    return () => {
      window.removeEventListener('fox:model-service-changed', all)
      window.removeEventListener('focus', regained)
      document.removeEventListener('visibilitychange', visible)
    }
  }, [recheck])
  if (!client.agentBackendCredentials) return null

  const piSource = probes.pi?.probe?.source ?? null
  const piNative = piSource?.current === 'native' || piSource?.current === 'reused_native'
  const choosePiSource = async (next: 'fox' | 'native') => {
    const target: AgentBackendTarget = { backend: 'pi', fullAccess: false }
    setSourceBusy(true)
    setSourceError(null)
    try {
      if (next === 'fox') {
        if (!client.agentBackendChooseFoxSource) return
        await client.agentBackendChooseFoxSource(target, true)
      } else {
        if (!client.agentBackendImportCandidates || !client.agentBackendImportCredentials) return
        const items = await client.agentBackendImportCandidates(target)
        await client.agentBackendImportCredentials(target, items.map((item) => item.id), true, true, true)
      }
      recheck(['pi'])
    } catch {
      setSourceError(next === 'fox' ? '没能切换到 Fox 模型服务，请重试。' : '没能切换到本机 ~/.pi，请确认本机已有 Pi 的配置后重试。')
    } finally {
      setSourceBusy(false)
    }
  }

  return (
    <>
      {EXTERNAL_AGENT_IDS.map((backend) => {
        const entry = probes[backend]
        const state: ExternalAgentState = checking[backend] ? 'checking' : externalAgentState(entry?.probe, entry?.connection, entry?.failed)
        const label = EXTERNAL_AGENT_SETTING_LABELS[backend]
        const on = isAgentEnabled(backend, enabled)
        return (
          <SettingsRow
            key={backend}
            className="fox-external-agent-row"
            title={label}
            description={<span className="fox-external-agent-state" data-external-agent-state={state}><i className={state === 'detected' ? 'fox-settings-dot is-ok' : 'fox-settings-dot is-empty'} aria-hidden="true" />{EXTERNAL_AGENT_STATE_LABELS[state]}</span>}
          >
            {onOpenModelSettings && <Button type="button" variant="ghost" size="sm" onClick={onOpenModelSettings}>去模型服务配置</Button>}
            <Switch checked={on} aria-label={`启用 ${label}`} data-external-agent={backend} onCheckedChange={(value) => {
              setExternalAgentEnabled(backend, value)
              // Turning an Agent on checks it at once (「检测中」 until the Host answers).
              if (value) recheck([backend])
            }} />
          </SettingsRow>
        )
      })}
      {piSource && (piSource.nativeAvailable || piNative) && (
        <SettingsRow className="fox-external-agent-row" title="Pi 配置来源" description={piSource.sourceChanged ? '本机 ~/.pi 的配置已变化，请重新选择。' : '没选过时默认用 Fox 模型服务的 Key。'}>
          <Select value={piNative ? 'native' : 'fox'} disabled={sourceBusy} onValueChange={(value) => void choosePiSource(value === 'native' ? 'native' : 'fox')}>
            <SelectTrigger className="fox-settings-agent-select" aria-label="Pi 配置来源"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="fox">Fox 模型服务</SelectItem>
              <SelectItem value="native" disabled={!piSource.nativeAvailable}>本机 ~/.pi</SelectItem>
            </SelectContent>
          </Select>
        </SettingsRow>
      )}
      {sourceError && <p className="fox-settings-error fox-agent-backend-settings-error" role="alert">{sourceError}</p>}
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
    <SettingsGroup title="Agent 驱动" className="fox-agent-backend-settings" aside={<span>外部 Agent 需要你先在电脑上安装并配置好，启用后才会出现在对话的 Agent 选择里</span>}>
      <ExternalAgentRows client={client} enabled={enabled} onOpenModelSettings={onOpenModelSettings} />
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
