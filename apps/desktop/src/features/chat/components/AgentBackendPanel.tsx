// Settings batch (13.15 re-test fixes ① ② + item 4): everything the
// composer's top slot shows for the outer Agent backend, with its own data
// loading, so the rules can be tested without the workbench.
//
// * The backend of a NEW conversation comes from 设置 →「新会话使用的 Agent」
//   (`draftBackend`), never from a control here; an open conversation shows
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
import { AgentBackendCredentialCard } from './AgentBackendCredentialCard'
import {
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
import { refreshesAgentBackendCapabilities, type AgentBackendCapabilities, type AgentBackendId, type AgentBackendStatus } from '@/features/conversations/model/agent-backend'

/** The Host calls the panel needs (a subset of `desktopClient`). */
export interface AgentBackendPanelClient {
  agentBackendCapabilities: (backend: AgentBackendId, conversationId?: string | null) => Promise<AgentBackendCapabilities>
  agentBackendPermissionStatus: (conversationId?: string | null) => Promise<AgentBackendPermissionStatus>
  agentBackendCredentials: (target: AgentBackendTarget) => Promise<CredentialProbe>
  agentBackendImportCandidates: (target: AgentBackendTarget) => Promise<ImportItem[]>
  agentBackendImportCredentials: (target: AgentBackendTarget, selected: string[], allProjects: boolean, overwrite?: boolean) => Promise<ImportReport>
  agentBackendOpenPrivateHome: (target: AgentBackendTarget) => Promise<unknown>
  agentBackendRevokeFullAccess: (conversationId: string) => Promise<unknown>
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
  draftBackend,
  draftProjectId = null,
  draftProjectRoot = null,
  draftFullAccess = false,
  refusal = null,
  sessionTick = 0,
}: {
  client: AgentBackendPanelClient
  status: AgentBackendStatus | null
  /** The open conversation's frozen backend; null for a new conversation. */
  lockedBackend: AgentBackendId | null
  conversationId: string | null
  /** Backend of a new conversation (设置 →「新会话使用的 Agent」, after the administrator's switch). */
  draftBackend: AgentBackendId
  draftProjectId?: string | null
  draftProjectRoot?: string | null
  /** 设置 →「新外部会话使用完全访问」as the Host applies it (false under FORBIDDEN). */
  draftFullAccess?: boolean
  refusal?: string | null
  /** Bumped by the caller to refresh everything (e.g. after a run). */
  sessionTick?: number
}) {
  const shownBackend: AgentBackendId = lockedBackend ?? draftBackend
  const capabilityConversationId = lockedBackend ? conversationId : null
  const [eventTick, setEventTick] = useState(0)
  const [onboardingTick, setOnboardingTick] = useState(0)
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

  const external = shownBackend !== 'fox'
  const capabilityKey = external ? JSON.stringify([shownBackend, capabilityConversationId]) : ''
  const capabilities = useKeyedResource(capabilityKey, sessionTick + eventTick, useCallback((key: string) => {
    const [backend, id] = JSON.parse(key) as [AgentBackendId, string | null]
    return client.agentBackendCapabilities(backend, id)
  }, [client]))

  const permissionKey = external ? JSON.stringify([shownBackend, capabilityConversationId]) : ''
  const permission = useKeyedResource(permissionKey, sessionTick + eventTick + onboardingTick, useCallback((key: string) => {
    const [, id] = JSON.parse(key) as [AgentBackendId, string | null]
    return client.agentBackendPermissionStatus(id)
  }, [client]))

  // The draft's mode is the setting as the Host applies it (FORBIDDEN never
  // grants it); the probe of a draft uses it.
  const target = credentialProbeTarget({
    lockedBackend,
    conversationId,
    draftBackend,
    draftProjectId,
    draftProjectRoot,
    draftFullAccess,
  })
  const targetKey = credentialProbeTargetKey(target)
  const probe = useKeyedResource(targetKey, sessionTick + onboardingTick, useCallback((key: string) => client.agentBackendCredentials(JSON.parse(key) as AgentBackendTarget), [client]))

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
      .finally(() => setOnboardingTick((tick) => tick + 1))
  }, [client, capabilityConversationId])

  const card = useMemo(() => {
    if (!targetKey || !probe) return null
    const current: AgentBackendTarget = JSON.parse(targetKey)
    return (
      <AgentBackendCredentialCard
        key={targetKey}
        probe={probe}
        permission={permission}
        onOpenFolder={() => client.agentBackendOpenPrivateHome(current)}
        onLoadCandidates={() => client.agentBackendImportCandidates(current)}
        onImport={async (selected, allProjects, overwrite) => {
          const report = await client.agentBackendImportCredentials(current, selected, allProjects, overwrite)
          setReceipt({ key: targetKey, value: importReceiptLines(report) })
          setOnboardingTick((tick) => tick + 1)
          return report
        }}
      />
    )
  }, [client, targetKey, probe, permission])

  return (
    <AgentBackendBar
      status={status}
      lockedBackend={lockedBackend}
      draftBackend={draftBackend}
      draftFullAccess={draftFullAccess}
      refusal={refusal}
      capabilities={capabilities}
      permission={permission}
      onRevokeFullAccess={handleRevoke}
      credentialCard={card}
      importReceipt={visibleReceipt}
      onDismissImportReceipt={dismissReceipt}
    />
  )
}
