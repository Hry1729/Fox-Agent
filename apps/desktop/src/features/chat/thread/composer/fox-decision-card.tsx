import { QuestionPages } from '../../composer/question-answer-rows'
import { GoalProgress } from '../../components/GoalProgress'
import { RuntimeApprovalQueue } from '../../components/RuntimeApprovalPrompt'
import { type ComposerAccessMode, FOX_ACCESS_MODES } from '../../access-mode'
import { type GoalProgressData } from '../../hooks/use-goal-progress'
import { type RuntimeQuestion, type RuntimeQuestionRequest } from '@/features/conversations/model/pending-interactions'
import { type ApprovalDecision, type ApprovalRecord, type ProjectRecord, type TaskEvidenceRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
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
  const activeApproval = pendingApprovals[0]
  const activeQuestionRequest = questionRequest ?? null
  const planRevisionPending = Boolean(onResolvePlanRevision && goalProgressData?.planRevisions.some((plan) => plan.status === 'proposed'))
  const goalConfirmationPending = goalProgressData?.goal.status === 'proposed' && Boolean(onResolveWorkModeConfirmation)
  const workflowGatePending = Boolean(onResolveWorkflowGate && goalProgressData?.expertWorkflow?.gates.some((gate) => gate.status === 'pending'))
  if (!activeApproval && !activeQuestionRequest && !goalConfirmationPending && !planRevisionPending && !workflowGatePending) return null

  return <div className="fox-decision-card">
    {activeApproval && onResolveApproval && <div className="fox-decision-approval-list">
      <RuntimeApprovalQueue approvals={pendingApprovals} onResolve={onResolveApproval} />
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
  const [submitting, setSubmitting] = useState(false)
  const submittingRef = useRef(false)

  useEffect(() => {
    setSelections({})
    setOtherAnswers({})
    submittingRef.current = false
    setSubmitting(false)
  }, [request.runId])

  const selectOption = (question: RuntimeQuestion, value: string) => {
    setOtherAnswers(current => ({ ...current, [question.questionId]: '' }))
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

  const buildAnswer = () => {
    const answers: Record<string, string | string[]> = {}
    const lines: string[] = []
    for (const question of request.questions) {
      const selected = selections[question.questionId] ?? []
      if (selected.length) {
        answers[question.questionId] = question.multiSelect ? selected : selected[0]
        const labels = selected.map((value) => question.options.find((option) => option.value === value)?.label ?? value)
        lines.push(`${question.question}：${labels.join('、')}`)
      } else {
        const openAnswer = otherAnswers[question.questionId]?.trim()
        if (!question.allowOther || !openAnswer) return null
        answers[question.questionId] = openAnswer
        lines.push(`${question.question}：${openAnswer}`)
      }
    }
    return { answers, text: lines.join('\n') }
  }

  const submit = async () => {
    if (!onSubmitQuestion || submittingRef.current) return
    const answer = buildAnswer()
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

  return <QuestionPages key={request.runId} questions={request.questions} selections={selections} otherAnswers={otherAnswers} submitting={submitting}
    onSelect={selectOption} onOtherChange={(question, value) => {
      setOtherAnswers(current => ({ ...current, [question.questionId]: value }))
      setSelections(current => ({ ...current, [question.questionId]: [] }))
    }} onSubmit={() => void submit()} />
}

/** The four execution-mode labels shared by the composer menu (A1). */
export const FOX_PERMISSION_MODES: Array<[ComposerAccessMode, string]> = FOX_ACCESS_MODES.map(([value, , label]) => [value, label])
