// Per-agent native permission list (权限按 Agent 原生列出).
//
// The list, its names and every rule come from the Host
// (`agent_backend_native_permissions`, built by
// `fox_agent_backend::native_permission`): Codex shows「请求审批 / 帮我批准 /
// 完全访问权限」, dsh its presets, pi Fox's own answers. This file only turns
// the Host's view into picker rows and decides which confirmation a pick
// needs; the Host refuses anything the UI should not have offered.
import { FULL_ACCESS_LOCK_NOTE, FULL_ACCESS_RISK_POINTS, REVOKE_FULL_ACCESS_NOTICE } from './agent-backend-onboarding'
import { AGENT_BACKEND_LABELS, type AgentBackendId } from './agent-backend'

export type NativeSwitchTiming = 'live' | 'next_turn_restart' | 'host_answered'

export interface NativePermissionOption {
  id: string
  label: string
  description: string
  /** The Agent's own name of what is configured. */
  native: string
  /** This option is the Agent's full access (Fox's「完全访问」). */
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
  /** The sandboxed choice (the same as `current` unless in full access). */
  sandboxedMode?: string
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
}

export interface NativePermissionRequest {
  conversationId?: string | null
  backend?: string | null
  mode?: string
  /** 设置: remember the Agent's default sandboxed choice only. */
  defaultOnly?: boolean
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

/**
 * Picker rows in the Agent's own names. Disabled rows keep their reason
 * (Codex「帮我批准」off OpenAI, FORBIDDEN, full access mid-session).
 * `sandboxedOnly` (设置) leaves the full-access option out: it is the
 * 「新外部会话使用完全访问」switch there.
 */
export function nativePermissionRows(view: NativePermissionView | null | undefined, options: { sandboxedOnly?: boolean } = {}): NativePermissionRow[] {
  if (!view) return []
  return view.catalog.options
    .filter((option) => !(options.sandboxedOnly && option.fullAccess))
    .map((option) => {
      const locked = option.fullAccess && !view.draft && !view.fullAccess
      const reason = option.disabledReason ?? (locked ? (view.fullAccessLockedReason ?? FULL_ACCESS_LOCK_NOTE) : null)
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

/** The note shown next to the picker (when a change applies). */
export function nativeSwitchBadge(view: NativePermissionView | null | undefined): string | null {
  if (!view || view.draft) return null
  return view.catalog.switchTiming === 'next_turn_restart' ? NEXT_TURN_RESTART_BADGE : null
}

export type NativePermissionChange =
  | { kind: 'none' }
  | { kind: 'refused'; reason: string }
  /** A new conversation picks full access: the existing risk confirmation. */
  | { kind: 'confirm_full_access'; points: readonly string[] }
  /** Leaving full access mid-session: the existing revoke confirmation. */
  | { kind: 'confirm_revoke'; notice: string }
  | { kind: 'apply' }

/** What picking `mode` needs before the Host is asked. */
export function nativePermissionChange(view: NativePermissionView | null | undefined, mode: string): NativePermissionChange {
  if (!view || mode === view.current) return { kind: 'none' }
  const row = nativePermissionRows(view).find((item) => item.value === mode)
  if (!row) return { kind: 'refused', reason: '没有这个权限选项' }
  if (row.disabled) return { kind: 'refused', reason: row.reason ?? '不可用' }
  if (row.fullAccess) {
    // Mid-session full access is never offered (row disabled above).
    return view.draft ? { kind: 'confirm_full_access', points: FULL_ACCESS_RISK_POINTS } : { kind: 'none' }
  }
  if (!view.draft && view.fullAccess) {
    if (!view.fullAccessGranted) return { kind: 'refused', reason: '本会话的完全访问来自工程开关（FOX_EXTERNAL_FULL_ACCESS），不能在界面撤销' }
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
    case 'applied': return '已切换，当前会话立即生效'
    case 'next_turn': return `已保存，下一轮开始时重启 ${nativeBackendName(view)} 生效`
    case 'failed': return `切换未生效，下一轮会重试${view.appliedError ? `：${view.appliedError}` : ''}`
    default: return null
  }
}
