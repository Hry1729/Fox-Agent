// Four-agent plan v0.3.1, Host wiring (P6 minimal UI), settings batch:
// * the backend of a NEW conversation is chosen in 设置 →「新会话使用的
//   Agent」(and full access in「新外部会话使用完全访问」); the bar only shows
//   it, with no control to change it here;
// * an existing conversation shows its frozen backend as locked, and a
//   read-only notice when the Host would refuse to start its runtime;
// * status, capability chips, the missing-login card, the import receipt
//   and the mid-session "撤销完全访问" stay on the bar.
//
// Display only: every rule is enforced by the Host (`host_routing`), so a
// stale or tampered UI can never widen what runs.
import { Lock } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import {
  canRevokeFullAccess,
  FULL_ACCESS_LOCK_NOTE,
  permissionChipLabel,
  REVOKE_FULL_ACCESS_NOTICE,
  type AgentBackendPermissionStatus,
} from '@/features/conversations/model/agent-backend-onboarding'
import {
  AGENT_BACKEND_LABELS,
  agentBackendAccess,
  agentBackendLockNotice,
  canPickAgentBackend,
  agentBackendCapabilityChips,
  type AgentBackendCapabilities,
  type AgentBackendId,
  type AgentBackendStatus,
} from '@/features/conversations/model/agent-backend'
import './agent-backend-bar.css'

/** The four effective capability dimensions (external backend wiring batch). */
function CapabilityChips({ capabilities }: { capabilities: AgentBackendCapabilities | null }) {
  const chips = agentBackendCapabilityChips(capabilities)
  if (chips.length === 0) return null
  return (
    <span className="fox-agent-backend-capabilities" aria-label="后端能力声明">
      {chips.map((chip) => (
        <span key={chip.key} className="fox-agent-backend-capability" data-dimension={chip.key} title={chip.title}>{chip.label}</span>
      ))}
      {capabilities?.warning && <span className="fox-agent-backend-notice" role="note">{capabilities.warning}</span>}
    </span>
  )
}

/** UX batch ②: lock chip of the permission mode + "撤销完全访问" (warns that the backend restarts). */
function LockedPermission({ permission, onRevoke }: { permission: AgentBackendPermissionStatus | null; onRevoke?: () => void }) {
  const [confirming, setConfirming] = useState(false)
  const label = permissionChipLabel(permission)
  if (!label) return null
  return (
    <>
      <span className="fox-agent-backend-chip" data-permission={permission?.mode} title={FULL_ACCESS_LOCK_NOTE}>
        <Lock size={11} aria-hidden="true" />
        {label}
      </span>
      {onRevoke && canRevokeFullAccess(permission) && (confirming
        ? (
          <span className="fox-agent-backend-risk" role="alertdialog" aria-label="撤销完全访问">
            {REVOKE_FULL_ACCESS_NOTICE}
            <button type="button" onClick={() => { setConfirming(false); onRevoke() }}>撤销并重启到沙箱</button>
            <button type="button" onClick={() => setConfirming(false)}>取消</button>
          </span>
        )
        : <button type="button" className="fox-agent-backend-revoke" onClick={() => setConfirming(true)}>撤销完全访问</button>)}
    </>
  )
}

/**
 * The last import receipt, kept on the bar after the login card is gone (a
 * successful import turns the probe ready and the card disappears with its
 * own message). Names only; dismissible; the caller clears it after a while.
 */
function ImportReceiptNotice({ lines, onDismiss }: { lines: readonly string[]; onDismiss?: () => void }) {
  if (lines.length === 0) return null
  return (
    <div className="fox-agent-backend-receipt" role="status" aria-label="导入回执">
      {lines.map((line) => <span key={line}>{line}</span>)}
      {onDismiss && <button type="button" onClick={onDismiss} aria-label="关闭导入回执">知道了</button>}
    </div>
  )
}

/** Where the backend of a new conversation is chosen. */
export const NEW_CONVERSATION_BACKEND_HINT = '新会话使用的 Agent 在“设置 → 助手设置”中选择，创建后不可更换'

export function AgentBackendBar({ status, lockedBackend, draftBackend, draftFullAccess = false, refusal = null, capabilities = null, permission = null, onRevokeFullAccess, credentialCard = null, importReceipt = null, onDismissImportReceipt }: {
  status: AgentBackendStatus | null
  /** The frozen backend of the open conversation; null for a new conversation. */
  lockedBackend: AgentBackendId | null
  /** Backend a new conversation will use (from the settings, after the administrator's switch). */
  draftBackend: AgentBackendId
  /** Whether a new external conversation gets full access (from the settings, after FORBIDDEN). */
  draftFullAccess?: boolean
  /**
   * The Host's refusal of the last send in this view (`agent_backend_*`).
   * Shown as the ONE notice of the bar, replacing the generic lock notice,
   * so a refused send never produces two messages.
   */
  refusal?: string | null
  /** Effective capabilities of the shown external backend; null for Fox. */
  capabilities?: AgentBackendCapabilities | null
  /** Permission mode / lock / FORBIDDEN from the Host. */
  permission?: AgentBackendPermissionStatus | null
  onRevokeFullAccess?: () => void
  /** Login guidance card of the shown external backend (open conversation, or a new one before its first message). */
  credentialCard?: ReactNode
  /** Last successful import receipt, shown once, here. */
  importReceipt?: readonly string[] | null
  onDismissImportReceipt?: () => void
}) {
  const receiptNotice = importReceipt && importReceipt.length > 0
    ? <ImportReceiptNotice lines={importReceipt} onDismiss={onDismissImportReceipt} />
    : null
  const refusalNotice = refusal
    ? <span className="fox-agent-backend-notice fox-agent-backend-refusal" role="alert">{refusal}</span>
    : null
  if (lockedBackend) {
    const access = agentBackendAccess(lockedBackend, status)
    const notice = agentBackendLockNotice(lockedBackend, access, status?.switchEnv)
    // A Fox conversation keeps today's composer exactly: no extra row.
    if (lockedBackend === 'fox') return null
    return (
      <div className="fox-agent-backend-bar" data-access={access} role="status">
        <span className="fox-agent-backend-chip" title="会话创建时已确定，之后不可更换">
          <Lock size={11} aria-hidden="true" />
          后端：{AGENT_BACKEND_LABELS[lockedBackend]}（已锁定）
        </span>
        {access === 'runnable' && <LockedPermission permission={permission} onRevoke={onRevokeFullAccess} />}
        {refusalNotice ?? (notice && <span className="fox-agent-backend-notice">{notice}</span>)}
        {access === 'runnable' && <CapabilityChips capabilities={capabilities} />}
        {access === 'runnable' && credentialCard}
        {receiptNotice}
      </div>
    )
  }

  // New conversation: the backend comes from the settings. Fox keeps
  // today's composer (no row) unless a refusal has to be shown.
  const value: AgentBackendId = canPickAgentBackend(draftBackend, status) ? draftBackend : 'fox'
  if (value === 'fox') {
    return refusalNotice ? <div className="fox-agent-backend-bar" data-access="draft">{refusalNotice}</div> : null
  }
  const fullAccess = draftFullAccess && !permission?.fullAccessForbidden
  return (
    <div className="fox-agent-backend-bar" data-access="draft" aria-label="新会话的 Agent 后端">
      <span className="fox-agent-backend-chip" data-draft-backend={value} title={NEW_CONVERSATION_BACKEND_HINT}>
        新会话：{AGENT_BACKEND_LABELS[value]}
      </span>
      <span className="fox-agent-backend-chip" data-permission={fullAccess ? 'full_access' : 'sandboxed'} title={FULL_ACCESS_LOCK_NOTE}>
        权限：{fullAccess ? '完全访问' : '沙箱'}（来自设置）
      </span>
      {refusalNotice ?? <CapabilityChips capabilities={capabilities} />}
      {/* Same rule as a locked conversation: the login card shows before the
          first message (80b1622). */}
      {credentialCard}
      {receiptNotice}
    </div>
  )
}
