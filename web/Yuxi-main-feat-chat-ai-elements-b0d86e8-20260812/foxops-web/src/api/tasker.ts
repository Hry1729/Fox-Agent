import { request } from './request'

export const taskerApi = {
  getTask(taskId: string) {
    return request.get(`/api/tasks/${encodeURIComponent(taskId)}`)
  },

  listTasks(params: Record<string, unknown> = {}) {
    const query = new URLSearchParams()
    Object.entries(params).forEach(([key, value]) => {
      if (value !== undefined && value !== null && value !== '') query.set(key, String(value))
    })
    const suffix = query.toString() ? `?${query}` : ''
    return request.get(`/api/tasks${suffix}`)
  },

  cancelTask(taskId: string) {
    return request.post(`/api/tasks/${encodeURIComponent(taskId)}/cancel`, {})
  }
}
