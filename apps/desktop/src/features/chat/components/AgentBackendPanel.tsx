// Everything the composer's top slot shows for the outer Agent backend, with
// its own data loading, so the rules can be tested without the workbench.
//
// UI 1103: external Agents are opt-in (设置 → Agent). Their login and
// configuration live there; nothing about a login is shown above the
// composer any more, in normal or developer mode. What is left: the
// read-only notice of a locked conversation and, in developer mode, the
// locked-backend chip, the capability chips and the local preview.
//
// Every async result (capability chips, permission status) is stored WITH
// the key of the target it was asked for and only shown while that key is
// still the current one; a late answer for an older target is dropped.
import { useCallback, useEffect, useRef, useState } from 'react'
import { AgentBackendBar } from './AgentBackendBar'
import { AgentNetworkPanel } from './AgentNetworkPanel'
import type { AgentNetworkClient } from '@/features/conversations/model/agent-network'
import { useDeveloperUi } from '../developer-ui'
import type { AgentBackendPermissionStatus } from '@/features/conversations/model/agent-backend-onboarding'
import type { NativePermissionRequest, NativePermissionView } from '@/features/conversations/model/agent-native-permission'
import { agentBackendAccess, isAgentBackendHidden, isAgentBackendRetired, NEW_CONVERSATION_AGENT_BACKEND, refreshesAgentBackendCapabilities, type AgentBackendCapabilities, type AgentBackendId, type AgentBackendStatus } from '@/features/conversations/model/agent-backend'

/** The Host calls the panel needs (a subset of `desktopClient`). */
export interface AgentBackendPanelClient extends Partial<AgentNetworkClient> {
  agentBackendCapabilities: (backend: AgentBackendId, conversationId?: string | null) => Promise<AgentBackendCapabilities>
  agentBackendPermissionStatus: (conversationId?: string | null) => Promise<AgentBackendPermissionStatus>
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

  const handleRevoke = useCallback(() => {
    if (!capabilityConversationId) return
    client.agentBackendRevokeFullAccess(capabilityConversationId)
      .catch(() => {})
      .finally(() => {
        setOnboardingTick((tick) => tick + 1)
        setPermissionTick((tick) => tick + 1)
      })
  }, [client, capabilityConversationId])

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
      developerDetails={networkPanel}
      developerUi={developerUi}
    />
  )
}
