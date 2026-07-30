import { useMemo } from 'react'
import type { ConversationDetail, GoalRecord, WorkTaskRecord, TaskEvidenceRecord } from '@/features/conversations/model/types'

export interface GoalProgressData {
  goal: GoalRecord
  tasks: WorkTaskRecord[]
  evidence: TaskEvidenceRecord[]
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

    // Prefer the current Goal, then retain the newest completed graph after the
    // final assistant response or an App restart.
    const activeGoal = detail.goals.find(g => g.status === 'active' || g.status === 'blocked')
      ?? [...detail.goals]
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
      completedCount,
      totalCount,
      currentTask
    }
  }, [detail])
}
