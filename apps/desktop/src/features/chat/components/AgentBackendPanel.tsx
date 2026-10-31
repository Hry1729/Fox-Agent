// Settings batch (13.15 re-test fixes ① ② + item 4): everything the
// composer's top slot shows for the outer Agent backend, with its own data
// loading, so the rules can be tested without the workbench.
//
// * A new conversation uses the composer's selection (`draftBackend`);
//   an open conversation shows
//   its frozen backend (`lockedBackend`).
// * Every async result (capability chips, permission status, login probe) is
//   stored WITH the key of the target it was asked for and only shown while
//   that key is still the current one; a late answer for an older target is
//   dropped by request sequence. So switching backend / project never shows
//   the previous backend's card or chips, not even for one render.
// * The login card is keyed by its target (React `key`): a new target mounts
//   a fresh card with no local message / candidates of the old one.
// * A successful import's receipt is shown ONCE, on the bar (the card no
//   longer echoes it); "知道了" or 30 s removes the whole notice. An import
//   error still shows inside the card.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AgentBackendBar } from './AgentBackendBar'
import { AgentNetworkPanel } from './AgentNetworkPanel'
import type { AgentNetworkClient } from '@/features/conversations/model/agent-network'
import { AgentBackendCredentialCard } from './AgentBackendCredentialCard'
import { useDeveloperUi } from '../developer-ui'
import { AgentModelConnectionCard, type AgentModelConnection, type AgentModelConnectionRequest } from './AgentModelConnectionCard'
import {
  credentialCardModel,
  credentialProbeTarget,
  credentialProbeTargetKey,
  IMPORT_RECEIPT_VISIBLE_MS,
  importReceiptLines,
  type AgentBackendPermissionStatus,
  type AgentBackendTarget,
  type CredentialProbe,
  type ImportItem,
  type ImportReport,
} from '@/features/conversations/model/agent-backend-onboarding'
import type { NativePermissionRequest, NativePermissionView } from '@/features/conversations/model/agent-native-permission'
import { agentBackendAccess, isAgentBackendHidden, isAgentBackendRetired, NEW_CONVERSATION_AGENT_BACKEND, refreshesAgentBackendCapabilities, type AgentBackendCapabilities, type AgentBackendId, type AgentBackendStatus } from '@/features/conversations/model/agent-backend'

/** The Host calls the panel needs (a subset of `desktopClient`). */
export interface AgentBackendPanelClient extends Partial<AgentNetworkClient> {
  agentBackendModelConnection?: (request: AgentModelConnectionRequest) => Promise<AgentModelConnection>
  agentBackendCapabilities: (backend: AgentBackendId, conversationId?: string | null) => Promise<AgentBackendCapabilities>
  agentBackendPermissionStatus: (conversationId?: string | null) => Promise<AgentBackendPermissionStatus>
  agentBackendCredentials: (target: AgentBackendTarget) => Promise<CredentialProbe>
  agentBackendImportCandidates: (target: AgentBackendTarget) => Promise<ImportItem[]>
  agentBackendImportCredentials: (target: AgentBackendTarget, selected: string[], allProjects: boolean, overwrite?: boolean, reuseForNewProjects?: boolean) => Promise<ImportReport>
  agentBackendChooseFoxSource?: (target: AgentBackendTarget, remember: boolean) => Promise<unknown>
  agentBackendOpenPrivateHome: (target: AgentBackendTarget) => Promise<unknown>
  agentBackendRevokeFullAccess: (conversationId: string) => Promise<unknown>
  /** Per-agent native permission list; absent = the old full-access chip. */
  agentBackendNativePermissions?: (request: NativePermissionRequest) => Promise<NativePermissionView>
  agentBackendNativePermissionSet?: (request: NativePermissionRequest & { mode: string }) => Promise<NativePermissionView>
  listenRuntimeEvents: (handler: (event: { conversationId: string; event: { type: string } }) => void) => Promise<() => void>
}

interface Keyed<T> {
  key: string
  value: T
}

/**
 * Load `fetch(key)` whenever `key` (or `tick`) changes; only the answer to
 * the latest request is kept, and it is returned only while its key is the
 * current one. An empty key means "nothing to load".
 */
export function useKeyedResource<T>(key: string, tick: number, fetch: (key: string) => Promise<T>): T | null {
  const [state, setState] = useState<(Keyed<T> & { tick: number }) | null>(null)
  const sequence = useRef(0)
  const fetchRef = useRef(fetch)
  fetchRef.current = fetch
  useEffect(() => {
    const request = ++sequence.current
    if (!key) {
      setState(null)
      return
    }
    // Drop what was shown for another target right away (a refresh of the
    // same target keeps the current value until the new one arrives).
    setState((current) => (current && current.key === key ? current : null))
    fetchRef.current(key)
      .then((value) => { if (request === sequence.current) setState({ key, tick, value }) })
      .catch(() => { if (request === sequence.current) setState(null) })
  }, [key, tick])
  return state && state.key === key && state.tick === tick ? state.value : null
}

export function AgentBackendPanel({
  client,
  status,
  lockedBackend,
  conversationId,
  draftBackend: pickedDraftBackend,
  draftProjectId = null,
  draftProjectRoot = null,
  draftFullAccess = false,
  refusal = null,
  failure = null,
  sessionTick = 0,
  developerUi: developerUiProp,
}: {
  client: AgentBackendPanelClient
  status: AgentBackendStatus | null
  /** The open conversation's frozen backend; null for a new conversation. */
  lockedBackend: AgentBackendId | null
  conversationId: string | null
  /** Backend selected in the new-conversation composer. */
  draftBackend: AgentBackendId
  draftProjectId?: string | null
  draftProjectRoot?: string | null
  /** Legacy Fox full-access preference; kept only for older Hosts. */
  draftFullAccess?: boolean
  refusal?: string | null
  /** Plan C4: the open external conversation's last failure and its action. */
  failure?: import('@/features/conversations/model/agent-backend-failure').AgentBackendFailureHint | null
  /** Bumped by the caller to refresh everything (e.g. after a run). */
  sessionTick?: number
  /** Developer mode override (tests); defaults to the 设置 / FOX_DEV_UI switch. */
  developerUi?: boolean
}) {
  const developerSwitch = useDeveloperUi()
  const developerUi = developerUiProp ?? developerSwitch
  // A hidden or retired backend passed by an older caller is shown, probed
  // and created as the built-in route: no login / permission prompt of such a
  // driver is ever rendered for a new conversation. A locked (existing)
  // conversation keeps its own backend; a retired or switched-off one stays
  // read-only.
  const draftBackend: AgentBackendId = isAgentBackendHidden(pickedDraftBackend, status) || isAgentBackendRetired(pickedDraftBackend, status)
    ? NEW_CONVERSATION_AGENT_BACKEND
    : pickedDraftBackend
  const shownBackend: AgentBackendId = lockedBackend ?? draftBackend
  // Read-only (retired / switched-off) conversation: no runtime, so nothing is
  // loaded for it - no capability chips, no permission list, no login card.
  const external = shownBackend !== NEW_CONVERSATION_AGENT_BACKEND && agentBackendAccess(shownBackend, status) === 'runnable'
  const capabilityConversationId = external && lockedBackend ? conversationId : null
  const [eventTick, setEventTick] = useState(0)
  const [onboardingTick, setOnboardingTick] = useState(0)
  // Bumped after a permission change / full-access revoke: the capability
  // chips and the permission status are fetched again (13.19 fix ②: after a
  // revoke the chip must drop「完全访问」).
  const [permissionTick, setPermissionTick] = useState(0)
  useEffect(() => {
    const refresh = () => setOnboardingTick(tick => tick + 1)
    window.addEventListener('fox:model-service-changed', refresh)
    return () => window.removeEventListener('fox:model-service-changed', refresh)
  }, [])


  // Refresh the chips as soon as the backend's handshake settles.
  useEffect(() => {
    if (!capabilityConversationId) return
    let disposed = false
    let unlisten: (() => void) | null = null
    client.listenRuntimeEvents((notification) => {
      if (refreshesAgentBackendCapabilities(notification, capabilityConversationId)) setEventTick((tick) => tick + 1)
    }).then((stop) => { if (disposed) stop(); else unlisten = stop }).catch(() => {})
    return () => { disposed = true; unlisten?.() }
  }, [client, capabilityConversationId])

  const capabilityKey = external ? JSON.stringify([shownBackend, capabilityConversationId]) : ''
  const capabilities = useKeyedResource(capabilityKey, sessionTick + eventTick + permissionTick, useCallback((key: string) => {
    const [backend, id] = JSON.parse(key) as [AgentBackendId, string | null]
    return client.agentBackendCapabilities(backend, id)
  }, [client]))

  const permissionKey = external ? JSON.stringify([shownBackend, capabilityConversationId]) : ''
  const permission = useKeyedResource(permissionKey, sessionTick + eventTick + onboardingTick + permissionTick, useCallback((key: string) => {
    const [, id] = JSON.parse(key) as [AgentBackendId, string | null]
    return client.agentBackendPermissionStatus(id)
  }, [client]))

  // The draft's mode is the setting as the Host applies it (FORBIDDEN never
  // grants it); the probe of a draft uses it. Nothing of a read-only (retired)
  // conversation is probed: there is no runtime, so there is no login card.
  const target = external ? credentialProbeTarget({
    lockedBackend,
    conversationId,
    draftBackend,
    draftProjectId,
    draftProjectRoot,
    draftFullAccess,
  }) : null
  const targetKey = credentialProbeTargetKey(target)
  const modelTick = sessionTick + onboardingTick
  const [connectionOverride, setConnectionOverride] = useState<(Keyed<AgentModelConnection> & { tick: number }) | null>(null)
  const loadedConnection = useKeyedResource(client.agentBackendModelConnection ? targetKey : '', modelTick, useCallback((key: string) => client.agentBackendModelConnection!({ target: JSON.parse(key) as AgentBackendTarget }).catch((error): AgentModelConnection => ({ status: 'verification_failed', providerId: null, providerName: null, apiType: '', candidates: [], message: error instanceof Error ? error.message : String(error) })), [client]))
  const modelConnection = connectionOverride?.key === targetKey && connectionOverride.tick === modelTick ? connectionOverride.value : loadedConnection
  const probe = useKeyedResource(targetKey, sessionTick + onboardingTick + (modelConnection?.status === 'ready' ? 1 : 0), useCallback((key: string) => client.agentBackendCredentials(JSON.parse(key) as AgentBackendTarget), [client]))

  const [receipt, setReceipt] = useState<Keyed<string[]> | null>(null)
  useEffect(() => {
    if (!receipt) return
    const timer = setTimeout(() => setReceipt(null), IMPORT_RECEIPT_VISIBLE_MS)
    return () => clearTimeout(timer)
  }, [receipt])
  // A receipt belongs to the target it was made for.
  useEffect(() => { setReceipt((current) => (current && current.key !== targetKey ? null : current)) }, [targetKey])
  const visibleReceipt = receipt && receipt.key === targetKey ? receipt.value : null
  const dismissReceipt = useCallback(() => setReceipt(null), [])

  const handleRevoke = useCallback(() => {
    if (!capabilityConversationId) return
    client.agentBackendRevokeFullAccess(capabilityConversationId)
      .catch(() => {})
      .finally(() => {
        setOnboardingTick((tick) => tick + 1)
        setPermissionTick((tick) => tick + 1)
      })
  }, [client, capabilityConversationId])

  // The login card is user-facing while the login needs action; the fold of
  // a ready login (「Agent 配置来源与导入」) is a developer detail.
  const connectionNeedsAction = Boolean(targetKey && probe && !probe.source?.choiceRequired && modelConnection && modelConnection.status !== 'ready' && modelConnection.status !== 'existing_login')
  const loginReady = credentialCardModel(probe, permission, true)?.kind === 'ready' && !connectionNeedsAction
  const card = useMemo(() => {
    if (!targetKey || !probe) return null
    const current: AgentBackendTarget = JSON.parse(targetKey)
    const credentialCard = (
      <AgentBackendCredentialCard
        key={targetKey}
        probe={probe}
        permission={permission}
        onOpenFolder={() => client.agentBackendOpenPrivateHome(current)}
        onLoadCandidates={() => client.agentBackendImportCandidates(current)}
        onChooseFox={client.agentBackendChooseFoxSource ? async remember => {
          await client.agentBackendChooseFoxSource!(current, remember)
          setOnboardingTick(tick => tick + 1)
        } : undefined}
        onImport={async (selected, allProjects, overwrite, reuseForNewProjects) => {
          const report = await client.agentBackendImportCredentials(current, selected, allProjects, overwrite, reuseForNewProjects)
          setReceipt({ key: targetKey, value: importReceiptLines(report) })
          setOnboardingTick((tick) => tick + 1)
          return report
        }}
      />
    )
    if (!probe.source?.choiceRequired && targetKey && modelConnection && modelConnection.status !== 'ready' && modelConnection.status !== 'existing_login') {
      return <><AgentModelConnectionCard key={`${targetKey}:${modelConnection.providerId}`} backend={shownBackend} connection={modelConnection} onSave={async choice => {
        const value = await client.agentBackendModelConnection!({ target: JSON.parse(targetKey), providerId: modelConnection.providerId ?? undefined, ...choice })
        setConnectionOverride({ key: targetKey, tick: modelTick, value })
        if (value.status === 'ready' || value.status === 'existing_login') window.dispatchEvent(new Event('fox:model-service-changed'))
      }} /><details><summary>使用已有 Agent 登录</summary>{credentialCard}</details></>
    }
    return credentialCard
  }, [client, targetKey, probe, permission, modelConnection, shownBackend, modelTick])

  const networkPanel = developerUi && external && capabilityConversationId && client.agentBackendNetworkStatus && client.agentBackendPreviewAction
    ? <AgentNetworkPanel key={capabilityConversationId} client={client as AgentNetworkClient} conversationId={capabilityConversationId} /> : null
  return (
    <AgentBackendBar
      status={status}
      lockedBackend={lockedBackend}
      draftBackend={draftBackend}
      draftFullAccess={draftFullAccess}
      refusal={refusal}
      failure={lockedBackend ? failure : null}
      capabilities={capabilities}
      permission={permission}
      onRevokeFullAccess={handleRevoke}
      credentialCard={loginReady ? null : card}
      developerDetails={(loginReady && card) || networkPanel ? <>{loginReady ? card : null}{networkPanel}</> : null}
      developerUi={developerUi}
      importReceipt={visibleReceipt}
      onDismissImportReceipt={dismissReceipt}
    />
  )
}
