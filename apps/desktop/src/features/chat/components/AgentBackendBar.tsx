// Four-agent plan v0.3.1, Host wiring first batch (P6 minimal UI):
// * a new conversation can pick its outer Agent backend; while the Host switch
//   (FOX_EXTERNAL_BACKENDS) keeps external backends off, the picker is disabled
//   and says why;
// * an existing conversation shows its frozen backend as locked, and a
//   read-only notice when the Host would refuse to start its runtime.
//
// Display only: every rule is enforced by the Host (`host_routing`), so a
// stale or tampered UI can never widen what runs.
import { Lock } from 'lucide-react'
import {
  AGENT_BACKEND_IDS,
  AGENT_BACKEND_LABELS,
  agentBackendAccess,
  agentBackendLockNotice,
  agentBackendPickerDisabledReason,
  canPickAgentBackend,
  isAgentBackendId,
  type AgentBackendId,
  type AgentBackendStatus,
} from '@/features/conversations/model/agent-backend'
import './agent-backend-bar.css'

export function AgentBackendBar({ status, lockedBackend, draftBackend, onDraftBackendChange, refusal = null }: {
  status: AgentBackendStatus | null
  /** The frozen backend of the open conversation; null for a new conversation. */
  lockedBackend: AgentBackendId | null
  draftBackend: AgentBackendId
  onDraftBackendChange: (backend: AgentBackendId) => void
  /**
   * The Host's refusal of the last send in this view (`agent_backend_*`).
   * Shown as the ONE notice of the bar, replacing the generic lock / picker
   * notice, so a refused send never produces two messages.
   */
  refusal?: string | null
}) {
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
        {refusalNotice ?? (notice && <span className="fox-agent-backend-notice">{notice}</span>)}
      </div>
    )
  }

  const disabledReason = agentBackendPickerDisabledReason(status)
  const value = canPickAgentBackend(draftBackend, status) ? draftBackend : 'fox'
  const option = status?.options.find((item) => item.backend === value)
  const pickedNotice = value !== 'fox' && option && !option.wired ? option.reason : null
  // With the switch off the control stays visible but disabled, with the
  // reason next to it (plan 6.5: unsupported settings are disabled and
  // explained, never silently hidden or swapped).
  return (
    <div className="fox-agent-backend-bar" data-access="draft">
      <label className="fox-agent-backend-picker" title={disabledReason ?? '新会话使用的 Agent 后端，创建后不可更换'}>
        <span>Agent 后端</span>
        <select
          aria-label="新会话的 Agent 后端"
          value={value}
          disabled={Boolean(disabledReason)}
          onChange={(event) => {
            const next = event.target.value
            if (isAgentBackendId(next) && canPickAgentBackend(next, status)) onDraftBackendChange(next)
          }}
        >
          {AGENT_BACKEND_IDS.map((backend) => {
            const entry = status?.options.find((item) => item.backend === backend)
            const enabled = backend === 'fox' || Boolean(entry?.enabled)
            return (
              <option key={backend} value={backend} disabled={!enabled} title={entry?.reason ?? undefined}>
                {AGENT_BACKEND_LABELS[backend]}{entry?.experimental ? '（实验）' : ''}{enabled ? '' : '（未启用）'}
              </option>
            )
          })}
        </select>
      </label>
      {refusalNotice ?? <>
        {disabledReason && <span className="fox-agent-backend-notice">{disabledReason}</span>}
        {pickedNotice && <span className="fox-agent-backend-notice">{pickedNotice}</span>}
      </>}
    </div>
  )
}
