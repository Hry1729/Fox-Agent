// Per-agent permission list mapped onto Fox's four tiers (plan C2).
//
// The list, its names and every rule come from the Host
// (`agent_backend_native_permissions`, built by
// `fox_agent_backend::native_permission`): every external Agent lists
//「只读 / 询问 / 自动 / 完全访问」, each bound to its own native mode (Codex
// has no 自动: greyed with the reason). pi is answered by Fox's approval
// engine, Codex and dsh by their own sandbox and approval. This file only
// turns the Host's view into picker rows, says when a change applies, and
// decides which confirmation a pick needs; the Host refuses anything the UI
// should not have offered.
import { FULL_ACCESS_RISK_POINTS, REVOKE_FULL_ACCESS_NOTICE } from './agent-backend-onboarding'
import { AGENT_BACKEND_LABELS, type AgentBackendId } from './agent-backend'

export type NativeSwitchTiming = 'live' | 'next_turn_restart' | 'host_answered'

/** Fox's four tiers (Host `native_permission::FoxTier`). */
export type FoxPermissionTier = 'read_only' | 'ask' | 'auto' | 'full_access'

export interface NativePermissionOption {
  id: string
  /** The Fox tier this option is; absent from older Hosts. */
  tier?: FoxPermissionTier
  label: string
  description: string
  /** The Agent's own name of what is configured. */
  native: string
  /** This option is the Agent's own full-access mode. */
  fullAccess: boolean
  disabledReason: string | null
}

export interface NativePermissionCatalog {
  backend: string
  /** The Agent's display name (Host `native_permission::display_name`: Codex / DeepSeek / pi). */
  displayName?: string
  options: NativePermissionOption[]
  defaultMode: string
  switchTiming: NativeSwitchTiming
  switchNote: string
}

/** `agent_backend_native_permissions` / `agent_backend_native_permission_set`. */
export interface NativePermissionView {
  catalog: NativePermissionCatalog
  current: string
  /** The non-full-access choice (the same as `current` unless in full access). */
  restrictedMode?: string
  fullAccess: boolean
  fullAccessGranted: boolean
  fullAccessForbidden: boolean
  /** A new conversation's list (no conversation yet). */
  draft: boolean
  /** Why the full-access option cannot be picked now (mid-session). */
  fullAccessLockedReason: string | null
  /** Set only: how the change reached the session. */
  applied?: 'not_running' | 'applied' | 'next_turn' | 'failed' | string
  appliedError?: string
  /**
   * The conversation's (or draft's) project already confirmed full access
   * once (plan C2: the same first-time second confirmation as a Fox
   * conversation). Absent from older Hosts: confirm every time.
   */
  projectFullAccessConfirmed?: boolean
}

export interface NativePermissionRequest {
  conversationId?: string | null
  backend?: string | null
  mode?: string
  /** 设置: remember the Agent's default non-full-access choice only. */
  defaultOnly?: boolean
  /** A new conversation's project (the first full access per project is confirmed once). */
  projectId?: string | null
  /** The user confirmed the full-access warning. */
  confirmFullAccess?: boolean
}

export interface NativePermissionRow {
  value: string
  label: string
  title: string
  disabled: boolean
  reason: string | null
  fullAccess: boolean
}

/** dsh: a change waits for the restart at the next turn. */
export const NEXT_TURN_RESTART_BADGE = '下一轮重启生效'
/** Codex: `session/set_mode` on the live session. */
export const LIVE_SWITCH_BADGE = '切换立即生效'
/** pi: Fox's approval engine answers by the new tier. */
export const HOST_ANSWERED_BADGE = '下一次请求起生效'

/**
 * Picker rows in the Agent's own names. Disabled rows keep only the Host's
 * reason (e.g. Codex「帮我批准」off OpenAI, or FORBIDDEN). The Agent's own
 * full access is selectable in place; it is not a Fox-level mode.
 * `restrictedOnly` (设置) leaves the full-access option out of the default
 * permission rows.
 */
export function nativePermissionRows(view: NativePermissionView | null | undefined, options: { restrictedOnly?: boolean } = {}): NativePermissionRow[] {
  if (!view) return []
  return view.catalog.options
    .filter((option) => !(options.restrictedOnly && option.fullAccess))
    .map((option) => {
      const reason = option.disabledReason
      return {
        value: option.id,
        label: option.label,
        title: [option.description, option.native, reason].filter(Boolean).join(' · '),
        disabled: Boolean(reason),
        reason,
        fullAccess: option.fullAccess,
      }
    })
}

/** The note shown next to the picker: when a change applies (plan C2). */
export function nativeSwitchBadge(view: NativePermissionView | null | undefined): string | null {
  if (!view || view.draft) return null
  switch (view.catalog.switchTiming) {
    case 'next_turn_restart': return NEXT_TURN_RESTART_BADGE
    case 'live': return LIVE_SWITCH_BADGE
    case 'host_answered': return HOST_ANSWERED_BADGE
    default: return null
  }
}

export type NativePermissionChange =
  | { kind: 'none' }
  | { kind: 'refused'; reason: string }
  /** The Agent's native full access: the risk confirmation. */
  | { kind: 'confirm_full_access'; points: readonly string[] }
  /** Leaving the Agent's native full access: one confirmation. */
  | { kind: 'confirm_revoke'; notice: string }
  | { kind: 'apply' }

/** What picking `mode` needs before the Host is asked. */
export function nativePermissionChange(view: NativePermissionView | null | undefined, mode: string): NativePermissionChange {
  if (!view || mode === view.current) return { kind: 'none' }
  const row = nativePermissionRows(view).find((item) => item.value === mode)
  if (!row) return { kind: 'refused', reason: '没有这个权限选项' }
  if (row.disabled) return { kind: 'refused', reason: row.reason ?? '不可用' }
  if (row.fullAccess) {
    // The first full access in a project is confirmed once; the project
    // remembers it afterwards (the Host enforces the same rule).
    if (view.projectFullAccessConfirmed) return { kind: 'apply' }
    return { kind: 'confirm_full_access', points: FULL_ACCESS_RISK_POINTS }
  }
  if (!view.draft && view.fullAccess) {
    return { kind: 'confirm_revoke', notice: REVOKE_FULL_ACCESS_NOTICE }
  }
  return { kind: 'apply' }
}

/**
 * The Agent's name in UI text: the Host's `displayName`, else the backend
 * bar's label for the id (older Hosts), else the id itself.
 */
export function nativeBackendName(view: NativePermissionView | null | undefined): string {
  const catalog = view?.catalog
  if (!catalog) return ''
  return catalog.displayName?.trim() || AGENT_BACKEND_LABELS[catalog.backend as AgentBackendId] || catalog.backend
}

/** One line after a mid-session switch. */
export function nativeAppliedNotice(view: NativePermissionView | null | undefined): string | null {
  switch (view?.applied) {
    case 'applied': return view.catalog.switchTiming === 'host_answered' ? '已切换，从下一次请求起生效' : '已切换，当前会话立即生效'
    case 'next_turn': return `已保存，下一轮开始时重启 ${nativeBackendName(view)} 生效`
    case 'failed': return `切换未生效，下一轮会重试${view.appliedError ? `：${view.appliedError}` : ''}`
    default: return null
  }
}
