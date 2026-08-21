import { useMemo } from 'react'
import type { AcceptanceRecord, ConversationDetail, GoalRecord, PlanRevisionRecord, ReviewFindingRecord, WorkTaskRecord, TaskEvidenceRecord } from '@/features/conversations/model/types'

export interface GoalProgressData {
  goal: GoalRecord
  tasks: WorkTaskRecord[]
  evidence: TaskEvidenceRecord[]
  planRevisions: PlanRevisionRecord[]
  reviewFindings: ReviewFindingRecord[]
  acceptances: AcceptanceRecord[]
  completedCount: number
  totalCount: number
  currentTask: WorkTaskRecord | null
}

/**
 * Extract active Goal progress from ConversationDetail
 * Returns null if no active Goal exists
 */
export function useGoalProgress(detail: ConversationDetail | null): GoalProgressData | null {
  return useMemo(() => {
    if (!detail || !detail.goals || detail.goals.length === 0) {
      return null
    }

    // Status-only live events may briefly arrive without the Goal payload. They
    // are not renderable Goals and must not create an empty progress bar.
    const renderableGoals = detail.goals.filter((goal) => (
      goal.title.trim().length > 0 && goal.objective.trim().length > 0
    ))

    // Prefer the current Goal, then retain the newest completed graph after the
    // final assistant response or an App restart.
    const activeGoal = renderableGoals.find(g => g.status === 'proposed')
      ?? renderableGoals.find(g => g.status === 'active' || g.status === 'blocked')
      ?? [...renderableGoals]
        .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
        .find(g => g.status === 'completed')

    if (!activeGoal) {
      return null
    }

    // Filter tasks by goalId
    const goalTasks = (detail.tasks || []).filter(t => t.goalId === activeGoal.id)

    // Sort by ordinal
    goalTasks.sort((a, b) => a.ordinal - b.ordinal)

    // Filter evidence for these tasks
    const taskIds = new Set(goalTasks.map(t => t.id))
    const goalEvidence = (detail.evidence || []).filter(e => taskIds.has(e.taskId))

    // Calculate progress
    const completedCount = goalTasks.filter(t => t.status === 'completed').length
    const totalCount = goalTasks.length

    // Find current in_progress task
    const currentTask = goalTasks.find(t => t.status === 'in_progress') || null

    return {
      goal: activeGoal,
      tasks: goalTasks,
      evidence: goalEvidence,
      planRevisions: (detail.planRevisions || []).filter((item) => item.goalId === activeGoal.id),
      reviewFindings: (detail.reviewFindings || []).filter((item) => item.goalId === activeGoal.id),
      acceptances: (detail.acceptances || []).filter((item) => item.goalId === activeGoal.id),
      completedCount,
      totalCount,
      currentTask
    }
  }, [detail])
}
