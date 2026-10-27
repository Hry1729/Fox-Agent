// Four-agent plan v0.3.1, Host wiring (P6 minimal UI), settings batch:
// * the new conversation's backend follows its draft selection. External
//   Agents no longer get Fox's SRT wrapper, so the bar shows the Agent's own
//   native permission picker instead of a Fox sandbox / full-access chip.
// * an existing conversation shows its frozen backend as locked, and a
//   read-only notice when the Host would refuse to start its runtime;
// * status, capability chips, the missing-login card and the import receipt
//   stay on the bar.
//
// Display only: every rule is enforced by the Host (`host_routing`), so a
// stale or tampered UI can never widen what runs.
import { Lock } from 'lucide-react'
import { type ReactNode } from 'react'
import type { AgentBackendPermissionStatus } from '@/features/conversations/model/agent-backend-onboarding'
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

/** A neutral statement for older Hosts that do not send a native picker. */
function NoFoxSandboxNote() {
  return <span className="fox-agent-backend-chip" data-isolation="native">隔离：Fox 不额外包装（Agent 原生）</span>
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
export const NEW_CONVERSATION_BACKEND_HINT = '在输入区右下角选择本次新对话的 Agent，创建后不可更换'

export function AgentBackendBar({ status, lockedBackend, draftBackend, draftFullAccess = false, refusal = null, capabilities = null, permission = null, onRevokeFullAccess, permissionPicker = null, credentialCard = null, importReceipt = null, onDismissImportReceipt }: {
  status: AgentBackendStatus | null
  /** The frozen backend of the open conversation; null for a new conversation. */
  lockedBackend: AgentBackendId | null
  /** Backend selected for this new conversation. */
  draftBackend: AgentBackendId
  /** Retained for callers; external Workers now use their native permission only. */
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
  /**
   * The Agent's native permission list (when the Host offers it). Replaces
   * the sandbox / full-access chip; mid-session revoke goes through it.
   */
  permissionPicker?: ReactNode
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
        {access === 'runnable' && (permissionPicker ?? <NoFoxSandboxNote />)}
        {refusalNotice ?? (notice && <span className="fox-agent-backend-notice">{notice}</span>)}
        {access === 'runnable' && <CapabilityChips capabilities={capabilities} />}
        {access === 'runnable' && credentialCard}
        {receiptNotice}
      </div>
    )
  }

  // New conversation: display the selected draft backend. Fox keeps
  // today's composer (no row) unless a refusal has to be shown.
  const value: AgentBackendId = canPickAgentBackend(draftBackend, status) ? draftBackend : 'fox'
  if (value === 'fox') {
    return refusalNotice ? <div className="fox-agent-backend-bar" data-access="draft">{refusalNotice}</div> : null
  }
  return (
    <div className="fox-agent-backend-bar" data-access="draft" aria-label="新会话的 Agent 后端">
      <span className="fox-agent-backend-chip" data-draft-backend={value} title={NEW_CONVERSATION_BACKEND_HINT}>
        新会话：{AGENT_BACKEND_LABELS[value]}
      </span>
      {permissionPicker ?? <NoFoxSandboxNote />}
      {refusalNotice ?? <CapabilityChips capabilities={capabilities} />}
      {/* Same rule as a locked conversation: the login card shows before the
          first message (80b1622). */}
      {credentialCard}
      {receiptNotice}
    </div>
  )
}
