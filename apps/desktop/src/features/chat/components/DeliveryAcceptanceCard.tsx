// The card that asks the user to decide about ONE checklist item whose content
// the Host could not verify.
//
// This is deliberately NOT the tool-approval prompt. An approval grants
// authority to run something; accepting here grants nothing at all — it records
// that the user takes responsibility for content that is still unverified. The
// two must never share a surface, or "I accepted the file" starts reading as
// "I authorized the write".
//
// The card states the verification result and the acceptance as two separate
// facts, and never renders an accepted item as passed.

import { useCallback, useEffect, useState } from 'react'
import { ShieldAlert, ShieldCheck, Check, X, RotateCcw } from 'lucide-react'
import type {
  DeliveryAcceptanceDecision,
  DeliveryAcceptanceOutcome,
  DeliveryAcceptanceState,
} from '@/features/conversations/model/types'

interface DeliveryAcceptanceCardProps {
  runId: string
  itemKey: string
  description: string
  loadState: (runId: string, itemKey: string) => Promise<DeliveryAcceptanceState>
  resolve: (
    runId: string,
    itemKey: string,
    decision: DeliveryAcceptanceDecision,
  ) => Promise<DeliveryAcceptanceOutcome>
}

/**
 * A bounded, human-readable line for why accepting is not possible.
 *
 * `kind` is the closed token the Host returns; an unknown value falls back to
 * `detail`, and a missing `detail` falls back to a generic sentence. Neither
 * branch may claim the content passed or failed.
 */
function blockedReason(state: DeliveryAcceptanceState): string {
  if (!state.blocked) return ''
  switch (state.blocked.kind) {
    case 'row_not_pending':
      return '该交付行已由一次真实判决终结，不能再事后接受。'
    case 'not_a_verifiable_record':
      return '该行的耐久记录不可核验（缺信封、信封损坏或属于别的项），不能作为接受对象。'
    case 'class_not_unconfirmed':
      return `该行当前类别为 ${state.verificationClass}，只有“未核验”项可以被接受。`
    case 'blocked_by_fact':
      return '该行存在安全拒绝或合同不可用等风险，不能由用户代为承担。'
    case 'candidate_drift':
      return '候选文件已偏离接受时核验的字节，请重新核验后再决定。'
    case 'candidate_unreadable':
      return '候选文件当前无法在授权范围内读取，接受有效性未确认。'
    case 'version_drift':
      return '候选的受管版本已改变，请重新核验后再决定。'
    case 'contract_drift':
      return '该行的冻结合同已改变，请重新核验后再决定。'
    case 'authorization_withdrawn':
      return '项目授权已不在接受时的范围内，接受失效。'
    default:
      return state.blocked.detail || '当前无法接受该交付项。'
  }
}

export function DeliveryAcceptanceCard({
  runId,
  itemKey,
  description,
  loadState,
  resolve,
}: DeliveryAcceptanceCardProps) {
  const [state, setState] = useState<DeliveryAcceptanceState | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const refresh = useCallback(() => {
    let active = true
    void loadState(runId, itemKey)
      .then((next) => {
        if (active) {
          setState(next)
          setError(null)
        }
      })
      .catch((reason: unknown) => {
        if (active) setError(String(reason))
      })
    return () => {
      active = false
    }
  }, [loadState, runId, itemKey])

  useEffect(() => refresh(), [refresh])

  const act = useCallback(
    async (decision: DeliveryAcceptanceDecision) => {
      setBusy(true)
      setError(null)
      try {
        const outcome = await resolve(runId, itemKey, decision)
        // The Host returns the fresh row state, including any terminal the
        // acceptance just committed, so a stale local copy can never outlive it.
        setState(outcome.state)
      } catch (reason: unknown) {
        setError(String(reason))
      } finally {
        setBusy(false)
      }
    },
    [resolve, runId, itemKey],
  )

  if (error && !state) {
    return (
      <section className="fox-delivery-acceptance is-error" role="alert">
        <p>{error}</p>
      </section>
    )
  }
  if (!state) {
    return (
      <section className="fox-delivery-acceptance is-loading">
        <p>正在读取交付核验状态…</p>
      </section>
    )
  }

  const accepted = state.accepted !== null
  const blocked = state.blocked !== null && !accepted
  // R-1: a Run that already ended cannot take a NEW accept or reject — the Host
  // refuses those — so those buttons must not exist either. What it can still do
  // is revoke an acceptance made while it was parked, which is why the revoke
  // button is deliberately outside this guard.
  const mayDecide = !state.runTerminal

  return (
    <section className="fox-delivery-acceptance" aria-label={`交付接受：${description}`}>
      <header>
        <span className={accepted ? 'is-accepted' : 'is-unverified'}>
          {accepted ? <ShieldCheck size={15} /> : <ShieldAlert size={15} />}
        </span>
        <div>
          <strong>{description}</strong>
          <small>
            {accepted
              ? '用户已接受未核验风险 — 这不是核验通过'
              : '内容尚未核验 — 保留产物，等待你的决定'}
          </small>
        </div>
      </header>

      {/* The verification result, stated on its own and never overwritten by the
          acceptance. An accepted item still reads "unverified" here. */}
      <dl className="fox-delivery-acceptance-facts">
        <dt>核验结果</dt>
        <dd data-class={state.verificationClass}>{state.verificationClass}</dd>
        <dt>当前接受</dt>
        <dd>
          {accepted ? `已接受（${state.accepted?.acceptedBy}）` : state.rejected ? '已拒绝' : '未决定'}
        </dd>
      </dl>

      {blocked && <p className="fox-delivery-acceptance-blocked">{blockedReason(state)}</p>}
      {error && (
        <p className="fox-delivery-acceptance-error" role="alert">
          {error}
        </p>
      )}

      <div className="fox-delivery-acceptance-actions">
        {mayDecide && !accepted && !state.rejected && (
          <button type="button" disabled={busy || blocked} onClick={() => void act('accept')}>
            <Check size={14} />
            接受未核验风险
          </button>
        )}
        {mayDecide && !accepted && !state.rejected && (
          <button type="button" disabled={busy} onClick={() => void act('reject')}>
            <X size={14} />
            判定不可接受
          </button>
        )}
        {accepted && (
          <button type="button" disabled={busy} onClick={() => void act('revoke')}>
            <RotateCcw size={14} />
            撤销接受
          </button>
        )}
        {!mayDecide && !accepted && !state.rejected && (
          // The Run ended without an acceptance on this row. Said plainly rather
          // than hidden: an absent button reads as "nothing to do here", which is
          // not the same as "the opportunity has passed".
          <span className="fox-delivery-acceptance-closed">任务已结束，未对该项作出接受</span>
        )}
      </div>

      <p className="fox-delivery-acceptance-note">
        接受只记录你对该项未核验内容的取舍，不会把它改成“已通过”，也不会授予任何工具权限。
      </p>
    </section>
  )
}
