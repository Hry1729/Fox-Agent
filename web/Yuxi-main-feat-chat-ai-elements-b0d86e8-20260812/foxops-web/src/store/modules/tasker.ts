import { defineStore } from 'pinia'
import { taskerApi } from '@/api/tasker'

export interface TaskItem {
  id: string
  status?: string
  progress?: number
  message?: string
  error?: string
  [key: string]: unknown
}

export const useTaskerStore = defineStore('taskerStore', () => {
  const tasks = ref<Record<string, TaskItem>>({})
  const polling = ref(false)
  let timer: ReturnType<typeof setInterval> | null = null

  const upsertTask = (task: TaskItem) => {
    if (!task?.id) return
    tasks.value = { ...tasks.value, [task.id]: { ...tasks.value[task.id], ...task } }
  }

  const removeTask = (taskId: string) => {
    const next = { ...tasks.value }
    delete next[taskId]
    tasks.value = next
  }

  const fetchTask = async (taskId: string) => {
    const result: any = await taskerApi.getTask(taskId)
    const task = { id: taskId, ...(result || {}) }
    upsertTask(task)
    return task as TaskItem
  }

  const isTerminal = (status?: string) => {
    const value = (status || '').toLowerCase()
    return ['done', 'completed', 'success', 'failed', 'error', 'cancelled', 'canceled'].includes(
      value
    )
  }

  const startPolling = (intervalMs = 2000) => {
    if (timer) return
    polling.value = true
    timer = setInterval(async () => {
      const activeIds = Object.values(tasks.value)
        .filter((task) => !isTerminal(task.status))
        .map((task) => task.id)
      if (!activeIds.length) {
        stopPolling()
        return
      }
      await Promise.all(
        activeIds.map(async (id) => {
          try {
            await fetchTask(id)
          } catch {
            // 单任务失败不中断轮询
          }
        })
      )
    }, intervalMs)
  }

  const stopPolling = () => {
    if (timer) clearInterval(timer)
    timer = null
    polling.value = false
  }

  const trackTask = async (taskId: string) => {
    await fetchTask(taskId)
    startPolling()
  }

  const cancelTask = async (taskId: string) => {
    await taskerApi.cancelTask(taskId)
    await fetchTask(taskId)
  }

  return {
    tasks,
    polling,
    upsertTask,
    removeTask,
    fetchTask,
    trackTask,
    cancelTask,
    startPolling,
    stopPolling,
    isTerminal
  }
})
