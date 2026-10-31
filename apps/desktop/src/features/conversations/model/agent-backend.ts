// Outer Agent backend of a conversation (four-agent plan v0.3.1, 5.4 / 6.6;
// Host wiring first batch; single Pi mainline, product decision 2026-10-27).
// The Host owns every decision (`fox_agent_backend::host_routing`); this file
// only mirrors it for display, so the settings statement, the lock badge and
// the refusal notice say the same thing the Host will answer.
//
// This build supports exactly ONE driver for a new conversation: the built-in
// Fox route (`agent_backend = 'fox'`, model engine `pi`). The three external
// backends (`pi` / ACP, `codex`, `deepseek`) are retired: their stored records
// stay readable and their conversations stay openable, but no runtime is ever
// started for them and no new conversation can pick one.
//
// `agentBackend` is NOT the Kernel's `engineId` (`pi | codex | deepseek_harness`):
// an old conversation that used the Codex model adapter is still a `fox`
// conversation.

export type AgentBackendId = 'fox' | 'pi' | 'codex' | 'deepseek'

export const AGENT_BACKEND_IDS: readonly AgentBackendId[] = ['fox', 'pi', 'codex', 'deepseek']

export const AGENT_BACKEND_LABELS: Record<AgentBackendId, string> = {
  fox: 'Fox',
  pi: 'Pi',
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
 * Product decision 2026-10-24 hid Codex because of the outer SRT layer. That
 * layer now applies only to the Fox Agent, so Codex is offered again. Keep
 * this list and the Host's `AgentBackend::HIDDEN` in sync.
 */
export const HIDDEN_AGENT_BACKENDS: readonly AgentBackendId[] = []

/** Whether a backend is hidden from user-facing choices (local list or the Host's `hidden` flag). */
export function isAgentBackendHidden(backend: string, status?: { options: { backend: string; hidden?: boolean }[] } | null): boolean {
  if ((HIDDEN_AGENT_BACKENDS as readonly string[]).includes(backend)) return true
  return Boolean(status?.options.find((item) => item.backend === backend)?.hidden)
}

/**
 * Backends RETIRED by the single Pi mainline (product decision 2026-10-27).
 * Mirrors the Host's `host_routing::RETIRED_BACKENDS`: the ids stay readable
 * (an old conversation keeps showing its own binding), but nothing of theirs
 * is ever started and no new conversation can use one
 * (`agent_backend_retired`).
 */
export const RETIRED_AGENT_BACKEND_IDS: readonly AgentBackendId[] = ['pi', 'codex', 'deepseek']

/** The backends a NEW conversation can use: exactly one, the built-in Fox route. */
export const SUPPORTED_AGENT_BACKEND_IDS: readonly AgentBackendId[] = ['fox']

/**
 * The backend a new conversation is frozen to. `fox` is the storage id of the
 * built-in route, not a driver name: renaming it to `pi` would route a new
 * conversation to the retired external Pi ACP path.
 */
export const NEW_CONVERSATION_AGENT_BACKEND: AgentBackendId = 'fox'

/** Name of the driver the built-in Fox route runs (its model engine is pinned to Pi). */
export const BUILTIN_AGENT_DRIVER_LABEL = 'Pi'

/** What an Agent-backend mirror (status / preferences) may say about retirement. */
export interface AgentBackendRetirementMirror {
  /** Host `retiredBackends`; absent from older Hosts. */
  retiredBackends?: string[]
  /** Host options; an older Host only sends `enabled` (absent `retired` = `!enabled`). */
  options?: readonly { backend: string; enabled: boolean; hidden?: boolean; retired?: boolean }[]
}

/**
 * Whether one option is retired: what the Host says, or — for an older Host
 * that only sends `enabled` — everything that cannot be used. The built-in
 * route is never retired.
 */
export function agentBackendOptionRetired(option: { backend: string; enabled: boolean; retired?: boolean }): boolean {
  if (option.backend === 'fox') return false
  return option.retired ?? !option.enabled
}

/**
 * The retired ids of a Host mirror: the declared list, else the options
 * (absent `retired` = `!enabled`), else this build's own list.
 */
export function agentBackendRetiredIds(mirror?: AgentBackendRetirementMirror | null): AgentBackendId[] {
  const declared = (mirror?.retiredBackends ?? []).filter(isAgentBackendId)
  if (declared.length > 0) return declared
  const derived = (mirror?.options ?? [])
    .filter((option) => option.backend !== 'fox' && agentBackendOptionRetired(option))
    .map((option) => option.backend)
    .filter(isAgentBackendId)
  return derived.length > 0 ? derived : [...RETIRED_AGENT_BACKEND_IDS]
}

/**
 * Whether a backend is retired: this build's own list (the same list the Host
 * enforces), plus what the given Host mirror declares, so a newer Host that
 * retires another backend is mirrored too. Unknown → not retired, and
 * `agentBackendAccess` then still fails closed.
 */
export function isAgentBackendRetired(backend: string, mirror?: AgentBackendRetirementMirror | null): boolean {
  if (backend === 'fox') return false
  if ((RETIRED_AGENT_BACKEND_IDS as readonly string[]).includes(backend)) return true
  if (mirror?.retiredBackends?.includes(backend)) return true
  const option = mirror?.options?.find((item) => item.backend === backend)
  return option ? agentBackendOptionRetired(option) : false
}

/**
 * The backends a user can choose for a NEW conversation, in display order.
 * Exactly one in this build (the built-in Fox route): a select with one option
 * is not a choice, so no picker is rendered (see `ComposerAgentPicker`).
 */
export const SELECTABLE_AGENT_BACKEND_IDS: readonly AgentBackendId[] = AGENT_BACKEND_IDS
  .filter((backend) => !isAgentBackendHidden(backend))
  .filter((backend) => !isAgentBackendRetired(backend))
  .filter((backend) => (SUPPORTED_AGENT_BACKEND_IDS as readonly string[]).includes(backend))

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
  /**
   * The backend is retired in this build (product decision 2026-10-27):
   * readable history, never runnable. Returned for a request that names a
   * retired backend and for every runtime entry of a conversation frozen to
   * one.
   */
  retired: 'agent_backend_retired',
  // Refusals of an external send before anything ran (external backend wiring batch).
  turnBusy: 'agent_backend_turn_busy',
  attachmentsUnsupported: 'agent_backend_attachments_unsupported',
  projectRequired: 'agent_backend_project_required',
} as const

export interface AgentBackendOption {
  backend: AgentBackendId
  enabled: boolean
  /** Runtime wired in this build: false for every retired backend. */
  wired: boolean
  experimental: boolean
  /** Hidden from pickers (Host `AgentBackend::HIDDEN`); absent from older Hosts. */
  hidden?: boolean
  /** Retired in this build (readable history, no runtime); absent from older Hosts (= `!enabled`). */
  retired?: boolean
  reason: string | null
}

/** Result of the `agent_backends_status` command. */
export interface AgentBackendStatus {
  switchEnv: string
  anyExternalEnabled: boolean
  /** The backends a new conversation can use; absent from older Hosts (= only `fox`). */
  supportedBackends?: string[]
  /** Backends retired in this build; absent from older Hosts (= derived from `options`). */
  retiredBackends?: string[]
  options: AgentBackendOption[]
  unknownEntries: string[]
}

/**
 * How an existing conversation may be presented. `read_only_retired` is the
 * only read-only value this build produces (mirrors
 * `SessionAccess::ReadOnlyRetired`); the two others stay for older Hosts.
 */
export type AgentBackendAccess = 'runnable' | 'read_only_retired' | 'read_only_disabled' | 'read_only_not_wired'

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
  // A retired backend never runs, whatever a stored record or an older Host
  // says: history only, never re-driven by another driver.
  if (isAgentBackendRetired(backend, status)) return 'read_only_retired'
  const option = status?.options.find((item) => item.backend === backend)
  if (!option || !option.enabled) return 'read_only_disabled'
  return option.wired ? 'runnable' : 'read_only_not_wired'
}

/**
 * Text under the lock badge of an existing conversation; null for runnable.
 * A retired conversation keeps its own backend's name — it is never
 * relabelled as the built-in Pi driver.
 */
export function agentBackendLockNotice(backend: AgentBackendId, access: AgentBackendAccess, switchEnv = 'FOX_EXTERNAL_BACKENDS'): string | null {
  const label = AGENT_BACKEND_LABELS[backend]
  if (access === 'read_only_retired') {
    return `此会话绑定已退休的 Agent 后端「${label}」，只能查看历史，不会启动运行时，也不会改用其他驱动重跑。`
  }
  if (access === 'read_only_disabled') {
    return `此会话绑定 ${label} 后端，该后端当前未启用（${switchEnv}），只能查看历史，不会启动运行时。`
  }
  if (access === 'read_only_not_wired') {
    return `此会话绑定 ${label} 后端，本版本尚未接通其运行时，只能查看历史。`
  }
  return null
}

/**
 * Why a new conversation cannot choose a driver: this build has exactly one,
 * and every external one is retired (no environment variable, stored
 * preference or IPC call can bring one back).
 */
export function agentBackendPickerDisabledReason(status: AgentBackendStatus | null | undefined): string {
  const names = agentBackendRetiredIds(status).map((backend) => AGENT_BACKEND_LABELS[backend]).join(' / ')
  const retired = names ? `外部 Agent 后端（${names}）已退休；` : ''
  return `本版本只支持内置 ${BUILTIN_AGENT_DRIVER_LABEL} 驱动，${retired}新会话无法选择其他 Agent 后端。`
}

// ---------------------------------------------------------------------------
// Settings batch, items 4 / 5: 设置 →「新会话使用的 Agent」.
// Since the single Pi mainline that row is a plain statement, not a select:
// the saved value is still reported (`defaultBackend`) so an old preference can
// be explained, but `effectiveBackend` is always the built-in route and every
// external backend is retired. Existing conversations keep their own binding,
// shown and read-only.

export interface AgentBackendPreferenceOption {
  backend: AgentBackendId
  enabled: boolean
  experimental: boolean
  /** Retired in this build; absent from older Hosts (= `!enabled`). */
  retired?: boolean
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
  /** No external backend can be used; always true since the single Pi mainline. */
  forcedOff: boolean
  /** Backends retired in this build; absent from older Hosts (= derived from `options`). */
  retiredBackends?: string[]
  options: AgentBackendPreferenceOption[]
  unifiedLoginProvider: string | null
  codexResponses: CodexResponsesProbe | null
}

/** Window event fired after the settings change (detail: the new preferences). */
export const AGENT_BACKEND_PREFERENCES_EVENT = 'fox:agent-backend-preferences-changed'

/** Before the Host answered: the built-in route, nothing else. */
export const DEFAULT_AGENT_BACKEND_PREFERENCES: AgentBackendPreferences = {
  defaultBackend: NEW_CONVERSATION_AGENT_BACKEND,
  effectiveBackend: NEW_CONVERSATION_AGENT_BACKEND,
  newExternalFullAccess: false,
  effectiveFullAccess: false,
  fullAccessForbidden: false,
  forbiddenReason: null,
  switchEnv: 'FOX_EXTERNAL_BACKENDS',
  forcedOff: true,
  retiredBackends: [...RETIRED_AGENT_BACKEND_IDS],
  options: [],
  unifiedLoginProvider: null,
  codexResponses: null,
}

/**
 * The backend a NEW conversation uses. This build supports exactly one, so a
 * saved preference (or an older Host) can never bring a retired driver back:
 * the answer is always the built-in Fox route.
 */
export function newConversationBackend(preferences: AgentBackendPreferences | null | undefined): AgentBackendId {
  const value = preferences?.effectiveBackend
  if (isAgentBackendId(value) && !isAgentBackendHidden(value) && !isAgentBackendRetired(value, preferences)) return value
  return NEW_CONVERSATION_AGENT_BACKEND
}

/** The value the「新会话使用的 Agent」select shows: a saved retired backend shows as what is effective. */
export function agentBackendSettingValue(preferences: AgentBackendPreferences | null | undefined): AgentBackendId {
  const saved = preferences?.defaultBackend
  if (isAgentBackendId(saved) && !isAgentBackendHidden(saved) && !isAgentBackendRetired(saved, preferences)) return saved
  return newConversationBackend(preferences)
}

/**
 * Rows of the「新会话使用的 Agent」choice. Exactly one since the single Pi
 * mainline, which is why the settings page renders the statement instead of a
 * select (a select with one option is not a choice).
 */
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

/** The plain statement that replaced the「新会话使用的 Agent」select: one supported driver. */
export const NEW_CONVERSATION_BACKEND_STATEMENT = `新会话使用内置 ${BUILTIN_AGENT_DRIVER_LABEL} 驱动`

/**
 * The saved default names a backend this build retired: the saved preference
 * no longer applies to a new conversation, and old conversations of that
 * backend keep their own binding (shown, read-only).
 */
export function agentBackendSavedRetiredNotice(preferences: AgentBackendPreferences | null | undefined): string | null {
  const saved = preferences?.defaultBackend
  if (!isAgentBackendId(saved) || !isAgentBackendRetired(saved, preferences)) return null
  const label = AGENT_BACKEND_LABELS[saved]
  return `之前保存的「${label}」已退休，不再用于新会话；已有的 ${label} 会话仍按原绑定显示，只能查看历史。`
}

/** Explanation beside the statement: no external backend can be used in this build. */
export function agentBackendForcedOffNotice(preferences: AgentBackendPreferences | null | undefined): string | null {
  if (!preferences?.forcedOff) return null
  return `外部 Agent 后端已在本版本退休，新会话一律使用内置 ${BUILTIN_AGENT_DRIVER_LABEL} 驱动。`
}

/** 旧「新外部会话使用完全访问」设置的兼容状态；界面已不再渲染该开关。 */
export function fullAccessSettingState(preferences: AgentBackendPreferences | null | undefined): { checked: boolean; disabled: boolean; reason: string | null } {
  if (!preferences) return { checked: false, disabled: true, reason: '正在读取…' }
  if (preferences.fullAccessForbidden) return { checked: false, disabled: true, reason: preferences.forbiddenReason || '本机已禁止完全访问' }
  if (preferences.forcedOff) return { checked: preferences.newExternalFullAccess, disabled: true, reason: agentBackendForcedOffNotice(preferences) }
  return { checked: preferences.newExternalFullAccess, disabled: false, reason: null }
}

/** Whether a picked value can be used for a new conversation: only the supported path can. */
export function canPickAgentBackend(backend: AgentBackendId, status: AgentBackendStatus | null | undefined): boolean {
  if (isAgentBackendRetired(backend, status)) return false
  if (isAgentBackendHidden(backend, status)) return false
  if (backend === NEW_CONVERSATION_AGENT_BACKEND) return true
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
 * send. A conversation bound to a retired backend is read-only (history
 * only), so a send is never made just to be refused. A Fox conversation is
 * never locked here, and a retired one is never relabelled as the built-in
 * driver.
 */
export function agentBackendInputLockedReason(backend: AgentBackendId, status: AgentBackendStatus | null | undefined): string | null {
  if (backend === NEW_CONVERSATION_AGENT_BACKEND) return null
  const access = agentBackendAccess(backend, status)
  if (access === 'read_only_retired') return `此会话绑定已退休的 Agent 后端「${AGENT_BACKEND_LABELS[backend]}」，只能查看历史`
  if (access !== 'read_only_disabled') return null
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
// from the backend's measured declaration and the session's launch
// declaration). Display only.

export interface AgentBackendCapabilityDimension {
  summary: string
  detail: string
}

export interface AgentBackendCapabilities {
  backend: AgentBackendId
  /** `standard` (no full access; `sandboxed` in data written before plan B3) or `full_access`. */
  mode: 'standard' | 'full_access' | string
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
