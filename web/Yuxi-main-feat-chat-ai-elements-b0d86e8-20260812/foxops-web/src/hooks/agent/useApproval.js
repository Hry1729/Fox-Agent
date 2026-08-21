import { reactive } from 'vue'
import { normalizeQuestions } from '@/utils/questionUtils'

/**
 * 人工审批 composable（搬自 Yuxi composables/useApproval.js）
 *
 * 处理 ask_user_question_required / human_approval_required / interrupted 三种 HITL 场景。
 * 提取 pendingInterrupt 并暴露 approvalState 供 ApprovalCard 渲染。
 */

const APPROVAL_REQUIRED_STATUSES = new Set([
  'ask_user_question_required',
  'human_approval_required'
])

const extractQuestionPayload = (chunk) => {
  const interruptInfo = chunk?.interrupt_info || {}
  const rawQuestions = chunk?.questions || interruptInfo?.questions || []
  const source = chunk?.source || interruptInfo?.source || 'interrupt'
  const questions = normalizeQuestions(rawQuestions)

  return { questions, source }
}

export const extractPendingInterrupt = (chunk, threadId) => {
  const payload = extractQuestionPayload(chunk)
  if (!payload.questions.length) return null

  return {
    questions: payload.questions,
    source: payload.source,
    status: chunk?.status || '',
    threadId: chunk?.thread_id || threadId,
    parentRunId: chunk?.run_id || chunk?.parent_run_id || null
  }
}

export function useApproval({ getThreadState, fetchThreadMessages }) {
  const approvalState = reactive({
    showModal: false,
    questions: [],
    status: '',
    threadId: null,
    parentRunId: null
  })

  const applyInterruptToApprovalState = (pendingInterrupt, fallbackThreadId) => {
    approvalState.showModal = true
    approvalState.questions = pendingInterrupt.questions
    approvalState.status = pendingInterrupt.status || ''
    approvalState.threadId = pendingInterrupt.threadId || fallbackThreadId
    approvalState.parentRunId = pendingInterrupt.parentRunId || null
  }

  const clearApprovalState = () => {
    approvalState.showModal = false
    approvalState.questions = []
    approvalState.status = ''
    approvalState.threadId = null
    approvalState.parentRunId = null
  }

  /**
   * 在流式处理中被调用：当收到 ask_user_question_required / human_approval_required 时
   * 提取 pendingInterrupt，停止流式，拉取历史，弹出审批卡片。
   * @returns {Boolean} true 表示已处理（应停止流式）
   */
  const processApprovalInStream = (chunk, threadId, currentAgentId) => {
    if (!APPROVAL_REQUIRED_STATUSES.has(chunk.status)) return false

    const threadState = getThreadState(threadId)
    if (!threadState) return false

    const pendingInterrupt = extractPendingInterrupt(chunk, threadId)
    if (!pendingInterrupt) return false

    threadState.isStreaming = false
    threadState.pendingInterrupt = pendingInterrupt

    applyInterruptToApprovalState(pendingInterrupt, threadId)

    fetchThreadMessages({ threadId })

    return true
  }

  /**
   * 从 threadState 恢复审批状态（切换线程 / 页面恢复时）
   */
  const restoreInterruptFromThreadState = (threadId) => {
    const threadState = getThreadState(threadId)
    const pendingInterrupt = threadState?.pendingInterrupt
    if (!pendingInterrupt?.questions?.length) return false

    threadState.isStreaming = false
    threadState.replyLoadingVisible = false
    threadState.pendingRequestId = null
    applyInterruptToApprovalState(pendingInterrupt, threadId)
    return true
  }

  const hideApprovalState = () => {
    clearApprovalState()
  }

  const resetApprovalState = () => {
    const threadState = getThreadState(approvalState.threadId)
    if (threadState) {
      threadState.pendingInterrupt = null
    }
    clearApprovalState()
  }

  return {
    approvalState,
    processApprovalInStream,
    restoreInterruptFromThreadState,
    hideApprovalState,
    resetApprovalState
  }
}
