import { Confirmation, ConfirmationAction, ConfirmationActions, ConfirmationRequest, ConfirmationTitle } from '@/components/ai-elements/confirmation'
import { Suggestion, Suggestions } from '@/components/ai-elements/suggestion'
import { GoalProgress } from '../../components/GoalProgress'
import { RuntimeApprovalPrompt } from '../../components/RuntimeApprovalPrompt'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { isConfirmationQuestion } from '../../fox-shared'
import { type GoalProgressData } from '../../hooks/use-goal-progress'
import { type RuntimeQuestion, type RuntimeQuestionRequest } from '@/features/conversations/model/pending-interactions'
import { type ApprovalDecision, type ApprovalRecord, type ProjectRecord, type TaskEvidenceRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { CircleHelp } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

/**
 * The decision card (migration step 5D): when an approval, question, goal
 * confirmation, plan revision or workflow gate is pending, it replaces the
 * composer input area. Ported from the legacy Composer's runtime-controlled
 * decision block — the demo (no-Tauri) variants stay in the legacy composer.
 */
export function FoxDecisionCard({ questionRequest, onSubmitQuestion, runtimeApprovals, onResolveApproval, goalProgressData, onResolveWorkModeConfirmation, onResolvePlanRevision, onResolveWorkflowGate, onEvidenceClick }: {
  questionRequest?: RuntimeQuestionRequest | null
  onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  runtimeApprovals?: ApprovalRecord[]
  onResolveApproval?: (approvalId: string, decision: ApprovalDecision) => void | Promise<boolean>
  goalProgressData?: GoalProgressData | null
  onResolveWorkModeConfirmation?: (goalId: string, expectedVersion: number, approved: boolean) => Promise<boolean> | void
  onResolvePlanRevision?: (planRevisionId: string, decision: 'approved' | 'rejected') => Promise<boolean> | void
  onResolveWorkflowGate?: (workflowRunId: string, stageId: string, decision: 'approved' | 'rejected') => Promise<boolean> | void
  onEvidenceClick?: (evidence: TaskEvidenceRecord) => void
}) {
  const pendingApprovals = (runtimeApprovals ?? [])
    .filter((approval) => approval.status === 'pending')
    .sort((left, right) => left.requestedAt - right.requestedAt || left.id.localeCompare(right.id))
  const activeApproval = pendingApprovals[0]
  const activeQuestionRequest = questionRequest ?? null
  const planRevisionPending = Boolean(onResolvePlanRevision && goalProgressData?.planRevisions.some((plan) => plan.status === 'proposed'))
  const goalConfirmationPending = goalProgressData?.goal.status === 'proposed' && Boolean(onResolveWorkModeConfirmation)
  const workflowGatePending = Boolean(onResolveWorkflowGate && goalProgressData?.expertWorkflow?.gates.some((gate) => gate.status === 'pending'))
  if (!activeApproval && !activeQuestionRequest && !goalConfirmationPending && !planRevisionPending && !workflowGatePending) return null

  return <div className="fox-decision-card">
    {activeApproval && onResolveApproval && <div className="fox-decision-approval-list">
      <RuntimeApprovalPrompt key={activeApproval.id} approval={activeApproval} onResolve={onResolveApproval} />
    </div>}
    {!activeApproval && activeQuestionRequest && <FoxQuestionCard request={activeQuestionRequest} onSubmitQuestion={onSubmitQuestion} />}
    {!activeApproval && !activeQuestionRequest && (goalConfirmationPending || planRevisionPending || workflowGatePending) && goalProgressData && <div className="fox-goal-decision-content">
      <GoalProgress data={goalProgressData} defaultExpanded={false} onEvidenceClick={onEvidenceClick} onResolveConfirmation={onResolveWorkModeConfirmation} onResolvePlanRevision={onResolvePlanRevision} onResolveWorkflowGate={onResolveWorkflowGate} />
    </div>}
  </div>
}

/** Same selection state machine as the legacy composer question card. */
function FoxQuestionCard({ request, onSubmitQuestion }: { request: RuntimeQuestionRequest; onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean> }) {
  const [selections, setSelections] = useState<Record<string, string[]>>({})
  const [otherAnswers, setOtherAnswers] = useState<Record<string, string>>({})
  const [freeform, setFreeform] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const submittingRef = useRef(false)

  useEffect(() => {
    setSelections({})
    setOtherAnswers({})
    setFreeform('')
    submittingRef.current = false
    setSubmitting(false)
  }, [request.runId])

  const selectOption = (question: RuntimeQuestion, value: string) => {
    setSelections((current) => {
      const selected = current[question.questionId] ?? []
      return {
        ...current,
        [question.questionId]: question.multiSelect
          ? selected.includes(value) ? selected.filter((item) => item !== value) : [...selected, value]
          : [value],
      }
    })
  }

  const buildAnswer = (otherText: string) => {
    const answers: Record<string, string | string[]> = {}
    const lines: string[] = []
    for (const question of request.questions) {
      const selected = selections[question.questionId] ?? []
      if (selected.length) {
        answers[question.questionId] = question.multiSelect ? selected : selected[0]
        const labels = selected.map((value) => question.options.find((option) => option.value === value)?.label ?? value)
        lines.push(`${question.question}：${labels.join('、')}`)
      } else {
        const individualAnswer = otherAnswers[question.questionId]?.trim()
        const fallbackAnswer = request.questions.length === 1 ? otherText.trim() : ''
        const openAnswer = individualAnswer || fallbackAnswer
        if (!question.allowOther || !openAnswer) return null
        answers[question.questionId] = openAnswer
        lines.push(`${question.question}：${openAnswer}`)
      }
    }
    if (otherText.trim() && !Object.values(answers).some((value) => value === otherText.trim())) {
      lines.push(`其他补充：${otherText.trim()}`)
    }
    return { answers, text: lines.join('\n') }
  }

  const submit = async () => {
    if (!onSubmitQuestion || submittingRef.current) return
    const answer = buildAnswer(freeform)
    if (!answer) {
      toast.error('请先完成需要补充的事项')
      return
    }
    submittingRef.current = true
    setSubmitting(true)
    const submitted = await onSubmitQuestion(answer.text, answer.answers)
    if (!submitted) {
      submittingRef.current = false
      setSubmitting(false)
    }
  }

  return <div className="fox-question-decision-content">
    <header><span><CircleHelp size={15} /></span><div><strong>需要你补充一点信息</strong><small>回答后 Fox 会继续刚才的任务</small></div></header>
    <div className="fox-question-decision-list">
      {request.questions.map((question, index) => isConfirmationQuestion(question) ? (
        <Confirmation key={question.questionId} approval={{ id: question.questionId }} state="approval-requested" className="fox-question-confirmation">
          <ConfirmationRequest>
            <ConfirmationTitle>{question.question}</ConfirmationTitle>
            <ConfirmationActions>
              {question.options.map((option) => <ConfirmationAction key={option.value} disabled={submitting} variant={(selections[question.questionId] ?? []).includes(option.value) ? 'default' : 'outline'} onClick={() => selectOption(question, option.value)}>{option.label}</ConfirmationAction>)}
            </ConfirmationActions>
          </ConfirmationRequest>
        </Confirmation>
      ) : (
        <section key={question.questionId} className="fox-question-field">
          <div><b>{request.questions.length > 1 ? `${index + 1}. ` : ''}{question.question}</b>{question.multiSelect && <small>可多选</small>}</div>
          <Suggestions className="fox-question-suggestions">
            {question.options.map((option) => <Suggestion key={option.value} suggestion={option.value} disabled={submitting} variant={(selections[question.questionId] ?? []).includes(option.value) ? 'default' : 'outline'} onClick={() => selectOption(question, option.value)}>{option.label}</Suggestion>)}
          </Suggestions>
          {question.allowOther && request.questions.length > 1 && <Input disabled={submitting} value={otherAnswers[question.questionId] ?? ''} onChange={(event) => setOtherAnswers((current) => ({ ...current, [question.questionId]: event.target.value }))} placeholder="输入这一项的补充答案" />}
        </section>
      ))}
    </div>
    <div className="fox-question-decision-actions">
      {request.questions.length === 1 && request.questions[0]?.allowOther && <Input disabled={submitting} value={freeform} onChange={(event) => setFreeform(event.currentTarget.value)} placeholder="也可以输入自己的答案" />}
      <Button disabled={submitting} onClick={() => void submit()}>{submitting ? '正在提交…' : '确认并继续'}</Button>
    </div>
  </div>
}

/** The permission-mode labels shared by the composer menu. */
export const FOX_PERMISSION_MODES: Array<[NonNullable<ProjectRecord['permissionMode']>, string]> = [
  ['ask', '执行前询问'],
  ['allow', '自动执行'],
  ['read_only', '只读模式'],
]
