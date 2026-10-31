// Four-agent plan v0.3.1, Host wiring (P6 minimal UI), settings batch; single
// Pi mainline (product decision 2026-10-27):
// * a new conversation has exactly one supported driver (the built-in Fox
//   route), so there is no draft row, no backend chip and no login or
//   permission prompt of a retired driver - only the Host's refusal is shown;
// * an existing conversation keeps its frozen backend, shown as locked. A
//   retired one is read-only and the notice names its own backend, never the
//   built-in Pi driver;
// * capability chips, the login card and the import receipt stay wired for the
//   records already made (nothing of theirs is loaded for a retired backend).
//
// Composer batch (2026-10-31): the bar is for what the user must act on
// (a refused send, a failed turn with its action, a read-only conversation,
// a missing login / model connection, an import receipt). The debugging
// details - the locked-backend chip, the capability chips, the configuration
// source / import fold of a ready login and the local preview - are shown
// only in developer mode (`developerUi`). The approval mode selector lives in
// the composer, next to the send button (one selector, not two).
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
  agentBackendCapabilityChips,
  type AgentBackendCapabilities,
  type AgentBackendId,
  type AgentBackendStatus,
} from '@/features/conversations/model/agent-backend'
import type { AgentBackendFailureHint } from '@/features/conversations/model/agent-backend-failure'
import './agent-backend-bar.css'

/**
 * Plan C4: why the last turn of this external Agent failed and what to do
 * (install / log in / upgrade / retry). Fox never falls back to another
 * driver; this is the whole answer.
 */
export function AgentBackendFailureNotice({ failure }: { failure: AgentBackendFailureHint }) {
  return (
    <div className="fox-agent-backend-failure" role="alert" data-action={failure.action} data-code={failure.code}>
      <strong>{failure.label}</strong>
      <span>{failure.detail}</span>
      <code>{failure.code}</code>
    </div>
  )
}

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

export function AgentBackendBar({ status, lockedBackend, draftBackend, draftFullAccess = false, refusal = null, failure = null, capabilities = null, permission = null, onRevokeFullAccess, permissionPicker: _permissionPicker = null, credentialCard = null, developerDetails = null, developerUi = false, importReceipt = null, onDismissImportReceipt }: {
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
  /** Plan C4: the last turn's failure of the open external conversation, with its action. */
  failure?: AgentBackendFailureHint | null
  /** Effective capabilities of the shown external backend; null for Fox. */
  capabilities?: AgentBackendCapabilities | null
  /** Permission mode / lock / FORBIDDEN from the Host. */
  permission?: AgentBackendPermissionStatus | null
  onRevokeFullAccess?: () => void
  /** Retained for callers; the approval selector now lives in the composer. */
  permissionPicker?: ReactNode
  /** Login guidance the user must act on (missing login / model connection). Always shown. */
  credentialCard?: ReactNode
  /** Developer-only details (configuration source / import of a ready login, local preview). */
  developerDetails?: ReactNode
  /** Developer mode: show the backend chip, capability chips and `developerDetails`. */
  developerUi?: boolean
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
    const runnable = access === 'runnable'
    const shownNotice = refusalNotice ?? (notice && <span className="fox-agent-backend-notice">{notice}</span>)
    const userCard = runnable ? credentialCard : null
    const devCard = runnable && developerUi ? developerDetails : null
    if (!developerUi && !shownNotice && !failure && !userCard && !receiptNotice) return null
    return (
      <div className="fox-agent-backend-bar" data-access={access} data-developer-ui={developerUi ? 'true' : undefined} role="status">
        {developerUi && (
          <span className="fox-agent-backend-chip" title="会话创建时已确定，之后不可更换">
            <Lock size={11} aria-hidden="true" />
            后端：{AGENT_BACKEND_LABELS[lockedBackend]}（已锁定）
          </span>
        )}
        {shownNotice}
        {developerUi && runnable && <CapabilityChips capabilities={capabilities} />}
        {failure && <AgentBackendFailureNotice failure={failure} />}
        {userCard}
        {devCard}
        {receiptNotice}
      </div>
    )
  }

  // New conversation: this build has exactly one supported driver, so there
  // is no draft chip and no login / permission prompt to render (a select with
  // one option is not a choice). Only a refused send is reported here.
  return refusalNotice ? <div className="fox-agent-backend-bar" data-access="draft">{refusalNotice}</div> : null
}
