// Outer Agent backend of a conversation (four-agent plan v0.3.1, 5.4 / 6.6;
// Host wiring first batch). The Host owns every decision
// (`fox_agent_backend::host_routing`); this file only mirrors it for display so
// the picker and the lock badge say the same thing the Host will answer.
//
// `agentBackend` is NOT the Kernel's `engineId` (`pi | codex | deepseek_harness`):
// an old conversation that used the Codex model adapter is still a `fox`
// conversation.

export type AgentBackendId = 'fox' | 'pi' | 'codex' | 'deepseek'

export const AGENT_BACKEND_IDS: readonly AgentBackendId[] = ['fox', 'pi', 'codex', 'deepseek']

export const AGENT_BACKEND_LABELS: Record<AgentBackendId, string> = {
  fox: 'Fox',
  pi: 'pi',
  codex: 'Codex',
  deepseek: 'DeepSeek',
}

/**
 * Backends that are implemented but hidden from every user-facing choice
 * (设置 →「新会话使用的 Agent」, the new-conversation / draft picker, the
 * per-Agent default permissions, the protocol editor's "used by" list).
 * Mirrors the Host's single switch `AgentBackend::HIDDEN`
 * (`crates/fox-agent-backend/src/port.rs`), which also refuses a new
 * conversation with it (`agent_backend_hidden`). Existing conversations of a
 * hidden backend still open, show their backend and run as before.
 *
 * Product decision 2026-10-24: built-in Agents are Fox, dsh and pi; Codex
 * returns through the planned custom ACP Agent entry. Remove the entry here
 * and in the Host to show it again.
 */
export const HIDDEN_AGENT_BACKENDS: readonly AgentBackendId[] = ['codex']

/** Whether a backend is hidden from user-facing choices (local list or the Host's `hidden` flag). */
export function isAgentBackendHidden(backend: string, status?: { options: { backend: string; hidden?: boolean }[] } | null): boolean {
  if ((HIDDEN_AGENT_BACKENDS as readonly string[]).includes(backend)) return true
  return Boolean(status?.options.find((item) => item.backend === backend)?.hidden)
}

/** The backends a user can choose for a NEW conversation, in display order. */
export const SELECTABLE_AGENT_BACKEND_IDS: readonly AgentBackendId[] = AGENT_BACKEND_IDS.filter((backend) => !isAgentBackendHidden(backend))

/** Stable error codes the Host returns for routing refusals. */
export const AGENT_BACKEND_ERROR_CODES = {
  invalid: 'agent_backend_invalid',
  disabled: 'agent_backend_disabled',
  misrouted: 'agent_backend_misrouted',
  /** No longer returned since the external backend wiring batch; kept for older Hosts. */
  notYetWired: 'agent_backend_not_yet_wired',
  entryUnsupported: 'agent_backend_entry_unsupported',
  lookupFailed: 'agent_backend_lookup_failed',
  /** A new conversation asked for a hidden backend (`HIDDEN_AGENT_BACKENDS`). */
  hidden: 'agent_backend_hidden',
  // Refusals of an external send before anything ran (external backend wiring batch).
  turnBusy: 'agent_backend_turn_busy',
  attachmentsUnsupported: 'agent_backend_attachments_unsupported',
  projectRequired: 'agent_backend_project_required',
} as const

export interface AgentBackendOption {
  backend: AgentBackendId
  enabled: boolean
  /** Runtime wired in this build (every backend since the external backend wiring batch). */
  wired: boolean
  experimental: boolean
  /** Hidden from pickers (Host `AgentBackend::HIDDEN`); absent from older Hosts. */
  hidden?: boolean
  reason: string | null
}

/** Result of the `agent_backends_status` command. */
export interface AgentBackendStatus {
  switchEnv: string
  anyExternalEnabled: boolean
  options: AgentBackendOption[]
  unknownEntries: string[]
}

export type AgentBackendAccess = 'runnable' | 'read_only_disabled' | 'read_only_not_wired'

export function isAgentBackendId(value: unknown): value is AgentBackendId {
  return typeof value === 'string' && (AGENT_BACKEND_IDS as readonly string[]).includes(value)
}

/**
 * The frozen backend of a conversation. A summary from an older Host has no
 * field: every pre-migration conversation is `fox` (plan 6.6), so that is
 * the mapping, not a guess. The database CHECK rules out any other value; if
 * one ever appears it is displayed as Fox here, while the Host still refuses
 * every runtime entry for it (`agent_backend_invalid`), so display can never
 * widen what runs.
 */
export function conversationAgentBackend(conversation: { agentBackend?: string | null } | null | undefined): AgentBackendId {
  const value = conversation?.agentBackend
  if (value === undefined || value === null) return 'fox'
  return isAgentBackendId(value) ? value : 'fox'
}

/** Mirrors `host_routing::session_access`. Unknown status fails closed. */
export function agentBackendAccess(backend: AgentBackendId, status: AgentBackendStatus | null | undefined): AgentBackendAccess {
  if (backend === 'fox') return 'runnable'
  const option = status?.options.find((item) => item.backend === backend)
  if (!option || !option.enabled) return 'read_only_disabled'
  return option.wired ? 'runnable' : 'read_only_not_wired'
}

/** Text under the lock badge of an existing conversation; null for runnable. */
export function agentBackendLockNotice(backend: AgentBackendId, access: AgentBackendAccess, switchEnv = 'FOX_EXTERNAL_BACKENDS'): string | null {
  const label = AGENT_BACKEND_LABELS[backend]
  if (access === 'read_only_disabled') {
    return `此会话绑定 ${label} 后端，该后端当前未启用（${switchEnv}），只能查看历史，不会启动运行时。`
  }
  if (access === 'read_only_not_wired') {
    return `此会话绑定 ${label} 后端，本版本尚未接通其运行时，只能查看历史。`
  }
  return null
}

/** Why only Fox can be picked; null when an external backend can be picked. */
export function agentBackendPickerDisabledReason(status: AgentBackendStatus | null | undefined): string | null {
  if (!status) return '正在读取可用的 Agent 后端…'
  if (!status.anyExternalEnabled) {
    const names = SELECTABLE_AGENT_BACKEND_IDS.filter((backend) => backend !== 'fox').map((backend) => AGENT_BACKEND_LABELS[backend]).join(' / ')
    return `外部 Agent 后端（${names}）已被管理员关闭（环境变量 ${status.switchEnv}）；新会话使用 Fox。`
  }
  return null
}

// ---------------------------------------------------------------------------
// Settings batch, items 4 / 5: 设置 →「新会话使用的 Agent」and
// 「新外部会话使用完全访问」(Host: `agent_backend_preferences_get/save`).
// Precedence: FOX_EXTERNAL_BACKENDS (administrator, can only force off) >
// this setting > Fox (default). Existing conversations keep their backend.

export interface AgentBackendPreferenceOption {
  backend: AgentBackendId
  enabled: boolean
  experimental: boolean
  reason: string | null
}

export interface CodexResponsesProbe {
  baseUrl: string
  supported: boolean | null
  status: number | null
  checkedAt: number
}

export interface AgentBackendPreferences {
  /** What the user saved. */
  defaultBackend: AgentBackendId
  /** What a new conversation uses now (Fox when the administrator forced the saved one off). */
  effectiveBackend: AgentBackendId
  newExternalFullAccess: boolean
  effectiveFullAccess: boolean
  fullAccessForbidden: boolean
  forbiddenReason: string | null
  switchEnv: string
  forcedOff: boolean
  options: AgentBackendPreferenceOption[]
  unifiedLoginProvider: string | null
  codexResponses: CodexResponsesProbe | null
}

/** Window event fired after the settings change (detail: the new preferences). */
export const AGENT_BACKEND_PREFERENCES_EVENT = 'fox:agent-backend-preferences-changed'

/** Before the Host answered: Fox, nothing else. */
export const DEFAULT_AGENT_BACKEND_PREFERENCES: AgentBackendPreferences = {
  defaultBackend: 'fox',
  effectiveBackend: 'fox',
  newExternalFullAccess: false,
  effectiveFullAccess: false,
  fullAccessForbidden: false,
  forbiddenReason: null,
  switchEnv: 'FOX_EXTERNAL_BACKENDS',
  forcedOff: false,
  options: [],
  unifiedLoginProvider: null,
  codexResponses: null,
}

/** The backend a NEW conversation uses (never an id the Host would refuse, never a hidden one). */
export function newConversationBackend(preferences: AgentBackendPreferences | null | undefined): AgentBackendId {
  const value = preferences?.effectiveBackend
  return isAgentBackendId(value) && !isAgentBackendHidden(value) ? value : 'fox'
}

/** The value the「新会话使用的 Agent」select shows: a saved hidden backend shows as what is effective (Fox). */
export function agentBackendSettingValue(preferences: AgentBackendPreferences | null | undefined): AgentBackendId {
  const saved = preferences?.defaultBackend
  if (isAgentBackendId(saved) && !isAgentBackendHidden(saved)) return saved
  return newConversationBackend(preferences)
}

/** Rows of the「新会话使用的 Agent」select: unavailable choices greyed with their reason. */
export function agentBackendSettingRows(preferences: AgentBackendPreferences | null | undefined): { value: AgentBackendId; label: string; disabled: boolean; reason: string | null }[] {
  return SELECTABLE_AGENT_BACKEND_IDS.map((backend) => {
    if (backend === 'fox') return { value: backend, label: AGENT_BACKEND_LABELS.fox, disabled: false, reason: null }
    const option = preferences?.options.find((item) => item.backend === backend)
    const disabled = !option?.enabled
    return {
      value: backend,
      label: `${AGENT_BACKEND_LABELS[backend]}${option?.experimental ? '（实验）' : ''}`,
      disabled,
      reason: disabled ? (option?.reason ?? '正在读取…') : null,
    }
  })
}

/** Explanation under the select when the administrator forced every external backend off. */
export function agentBackendForcedOffNotice(preferences: AgentBackendPreferences | null | undefined): string | null {
  if (!preferences?.forcedOff) return null
  return `管理员已通过环境变量 ${preferences.switchEnv} 关闭外部 Agent 后端，新会话一律使用 Fox。`
}

/** State of the「新外部会话使用完全访问」switch. */
export function fullAccessSettingState(preferences: AgentBackendPreferences | null | undefined): { checked: boolean; disabled: boolean; reason: string | null } {
  if (!preferences) return { checked: false, disabled: true, reason: '正在读取…' }
  if (preferences.fullAccessForbidden) return { checked: false, disabled: true, reason: preferences.forbiddenReason || '本机已禁止完全访问' }
  if (preferences.forcedOff) return { checked: preferences.newExternalFullAccess, disabled: true, reason: agentBackendForcedOffNotice(preferences) }
  return { checked: preferences.newExternalFullAccess, disabled: false, reason: null }
}

/** Whether a picked value can be used for a new conversation. */
export function canPickAgentBackend(backend: AgentBackendId, status: AgentBackendStatus | null | undefined): boolean {
  if (backend === 'fox') return true
  if (isAgentBackendHidden(backend, status)) return false
  return Boolean(status?.options.find((item) => item.backend === backend)?.enabled)
}

// ---------------------------------------------------------------------------
// Fix 1 after the Windows acceptance of 3a82f5d: a send the Host refuses with
// an `agent_backend_*` code started nothing (no message row, no run row), so
// the UI must not keep any optimistic trace of it, must not show the generic
// "处理失败" state, and shows exactly one refusal notice instead.

/** Outcome of `send` when the Host (or the read-only guard) refused it by backend. */
export const AGENT_BACKEND_REFUSED = 'agent_backend_refused' as const
export type AgentBackendRefusedOutcome = typeof AGENT_BACKEND_REFUSED

const REFUSAL_CODES = new Set<string>(Object.values(AGENT_BACKEND_ERROR_CODES))

/** Whether a Host error code is a routing refusal (nothing was started). */
export function isAgentBackendRefusalCode(code: string | null | undefined): boolean {
  return typeof code === 'string' && REFUSAL_CODES.has(code)
}

/** The single refusal notice the UI shows, scoped to one conversation (null = new-conversation draft). */
export interface AgentBackendRefusal {
  conversationId: string | null
  code: string
  message: string
}

/**
 * Why the composer input is disabled for this conversation; null when it may
 * send. Only an external conversation whose backend is switched off
 * (read-only) is locked, so a send is never made just to be refused. A Fox
 * conversation is never locked here.
 */
export function agentBackendInputLockedReason(backend: AgentBackendId, status: AgentBackendStatus | null | undefined): string | null {
  if (backend === 'fox') return null
  if (agentBackendAccess(backend, status) !== 'read_only_disabled') return null
  return `此会话绑定 ${AGENT_BACKEND_LABELS[backend]} 后端，该后端当前未启用，只能查看历史`
}

/**
 * Remove the optimistic trace of one refused send: the pending user message
 * and the pending run, restoring the run that was current before the send.
 * Everything else in the detail is left as it is.
 */
export function withoutRefusedSend<M extends { id: string }, R extends { id: string }, D extends { messages: M[]; lastRun: R | null }>(
  detail: D,
  pendingMessageId: string | null,
  previousLastRun: R | null,
): D {
  const messages = pendingMessageId ? detail.messages.filter((message) => message.id !== pendingMessageId) : detail.messages
  const lastRun = detail.lastRun && detail.lastRun.id.startsWith('pending-run-') ? previousLastRun : detail.lastRun
  if (messages === detail.messages && lastRun === detail.lastRun) return detail
  return { ...detail, messages, lastRun }
}

// ---------------------------------------------------------------------------
// External backend wiring batch: the four effective capability dimensions
// (`agent_backend_capabilities`, built by `fox_agent_backend::capability_view`
// from the backend's measured declaration and the session's sandbox
// declaration). Display only.

export interface AgentBackendCapabilityDimension {
  summary: string
  detail: string
}

export interface AgentBackendCapabilities {
  backend: AgentBackendId
  mode: 'sandboxed' | 'full_access' | string
  /** `live`: from this session's launch; `planned`: before the first launch. */
  source: 'live' | 'planned' | string
  approval: AgentBackendCapabilityDimension
  sandbox: AgentBackendCapabilityDimension
  cancel: AgentBackendCapabilityDimension
  observability: AgentBackendCapabilityDimension
  warning: string | null
}

/** The four chips in a fixed order (approval, sandbox, cancel, observability). */
export function agentBackendCapabilityChips(view: AgentBackendCapabilities | null | undefined): { key: string; label: string; title: string }[] {
  if (!view) return []
  const source = view.source === 'live' ? '' : '（启动前预估）'
  return (['approval', 'sandbox', 'cancel', 'observability'] as const).map((key) => ({
    key,
    label: view[key].summary,
    title: `${view[key].detail}${source}`,
  }))
}

/**
 * Run event the external runtime writes right after the backend's handshake
 * (`session/new` or `session/load` settled): from then on the backend's real
 * capability declaration is known, so the capability chips refresh on it
 * instead of waiting for the run to finish.
 */
export const AGENT_BACKEND_SESSION_EVENT = 'agent_backend.session' as const

/** Whether a runtime event should refresh the shown conversation's capability chips. */
export function refreshesAgentBackendCapabilities(
  notification: { conversationId: string; event: { type: string } } | null | undefined,
  conversationId: string | null | undefined,
): boolean {
  if (!notification || !conversationId) return false
  return notification.conversationId === conversationId && notification.event?.type === AGENT_BACKEND_SESSION_EVENT
}
