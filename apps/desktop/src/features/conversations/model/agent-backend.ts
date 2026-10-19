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

/** Stable error codes the Host returns for routing refusals. */
export const AGENT_BACKEND_ERROR_CODES = {
  invalid: 'agent_backend_invalid',
  disabled: 'agent_backend_disabled',
  misrouted: 'agent_backend_misrouted',
  notYetWired: 'agent_backend_not_yet_wired',
} as const

export interface AgentBackendOption {
  backend: AgentBackendId
  enabled: boolean
  /** Runtime wired in this build (only Fox in the first batch). */
  wired: boolean
  experimental: boolean
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

/** Why the new-conversation picker is disabled; null when something besides Fox can be picked. */
export function agentBackendPickerDisabledReason(status: AgentBackendStatus | null | undefined): string | null {
  if (!status) return '正在读取可用的 Agent 后端…'
  if (!status.anyExternalEnabled) {
    return `外部 Agent 后端（pi / Codex / DeepSeek）默认关闭，需由管理员设置环境变量 ${status.switchEnv} 后重启 Fox；新会话使用 Fox。`
  }
  return null
}

/** Whether a picked value can be used for a new conversation. */
export function canPickAgentBackend(backend: AgentBackendId, status: AgentBackendStatus | null | undefined): boolean {
  if (backend === 'fox') return true
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
