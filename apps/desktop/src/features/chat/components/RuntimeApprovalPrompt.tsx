// The prompt that asks the user to resolve ONE durable runtime approval.
//
// Extracted from workbench.tsx unchanged so the decision surface — including the
// purpose-specific whole-file replacement binding — can be mounted and clicked in
// a real DOM test. The identity it submits is `approval.id`, which for a Kernel
// approval is the versioned ticket the Host minted; the Host re-checks that exact
// ticket in the same transaction that consumes it, so the prompt must never
// invent, re-key, or reuse an approval identity of its own.
import { useRef, useState } from 'react'
import { Terminal } from 'lucide-react'
import {
  Confirmation,
  ConfirmationAction,
  ConfirmationActions,
  ConfirmationRequest,
  ConfirmationTitle,
} from '@/components/ai-elements/confirmation'
import { approvalPresentation } from '@/features/conversations/model/approval-presentation'
import {
  allowedApprovalDecisions,
  isRepairOverrideApproval,
  repairOverrideApprovalDetails,
  resolveAllowedApprovalDecision,
} from '@/features/conversations/model/approval-decision-policy'
import type { ApprovalDecision, ApprovalRecord } from '@/features/conversations/model/types'

export function RuntimeApprovalPrompt({
  approval,
  onResolve,
}: {
  approval: ApprovalRecord
  onResolve: (approvalId: string, decision: ApprovalDecision) => void | Promise<boolean>
}) {
  const request = approval.request
  const presentation = approvalPresentation(approval)
  const allowedDecisions = allowedApprovalDecisions(request)
  const repairOverride = isRepairOverrideApproval(request)
  const repairDetails = repairOverrideApprovalDetails(request)
  const [submitting, setSubmitting] = useState(false)
  const submittingRef = useRef(false)
  const resolve = async (decision: ApprovalDecision) => {
    if (submittingRef.current) return
    submittingRef.current = true
    setSubmitting(true)
    const resolved = await resolveAllowedApprovalDecision(approval, decision, onResolve)
    if (!resolved) {
      submittingRef.current = false
      setSubmitting(false)
    }
  }
  return (
    <Confirmation approval={{ id: approval.id }} state="approval-requested" className="fox-confirmation fox-runtime-confirmation">
      <ConfirmationRequest>
        <div className="fox-confirmation-body">
          <div className="fox-confirmation-content"><ConfirmationTitle>{presentation.title}</ConfirmationTitle><p><code>{presentation.target}</code></p><small>{presentation.summary}</small></div>
        </div>
        {repairOverride && <div className={`fox-approval-context ${repairDetails ? '' : 'is-invalid'}`}>
          {repairDetails
            ? <><p><strong>为什么还要再修一次：</strong>{repairDetails.rootCause}</p><p><strong>关联的审查问题：</strong>{repairDetails.findingIds.join('、')}</p></>
            : <p><strong>审批详情不完整。</strong>请先拒绝，并让 Fox 带上根因和关联审查问题重新发起。</p>}
        </div>}
        {presentation.command && <div className="fox-approval-command"><Terminal size={13} /><code>{presentation.command}</code></div>}
        {presentation.diff && <pre className="fox-approval-diff" aria-label="拟修改差异"><code>{presentation.diff}</code></pre>}
        {presentation.content !== undefined && <div className="fox-approval-context"><strong>拟写入内容</strong><pre className="fox-approval-diff" aria-label="拟写入内容"><code>{presentation.content || '（空文件）'}</code></pre></div>}
        {presentation.wholeFileReplacement && <div className="fox-approval-context fox-approval-replacement">
          <p><strong>用途：</strong>{presentation.wholeFileReplacement.purpose}（这不是普通写入审批；它只授权这一次整文件替换）</p>
          <p><strong>目标：</strong><code>{presentation.wholeFileReplacement.targetIdentity}</code></p>
          <p><strong>基线版本：</strong><code>{presentation.wholeFileReplacement.baselineVersion}</code></p>
          <p><strong>候选内容摘要：</strong><code>{presentation.wholeFileReplacement.candidateDigest}</code></p>
          <p><strong>请求摘要：</strong><code>{presentation.wholeFileReplacement.requestDigest}</code></p>
          <small>以上四项由 Host 在创建请求时绑定且不可变；确认后仅这一次派发可领取，且必须与这四项完全一致。</small>
        </div>}
        <ConfirmationActions className="fox-confirmation-actions">
          <ConfirmationAction variant="ghost" disabled={submitting} onClick={() => void resolve('deny')}>拒绝</ConfirmationAction>
          {allowedDecisions.includes('allow_once') && <ConfirmationAction variant="outline" disabled={submitting} onClick={() => void resolve('allow_once')}>只允许这一次</ConfirmationAction>}
          {allowedDecisions.includes('allow_conversation') && <ConfirmationAction disabled={submitting} onClick={() => void resolve('allow_conversation')}>{submitting ? '处理中…' : '本次对话始终允许'}</ConfirmationAction>}
        </ConfirmationActions>
      </ConfirmationRequest>
    </Confirmation>
  )
}
