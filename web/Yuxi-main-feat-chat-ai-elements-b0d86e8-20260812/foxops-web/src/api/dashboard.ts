import { request } from './request'

/**
 * 数据总览 API（管理员 / 超管）
 * 对接后端 /api/dashboard/*
 */
export const dashboardApi = {
  getConversations: (params: Record<string, any> = {}) => {
    const query = new URLSearchParams()
    Object.entries(params).forEach(([key, value]) => {
      if (value !== undefined && value !== null && value !== '') {
        query.append(key, String(value))
      }
    })
    const qs = query.toString()
    return request.get(qs ? `/api/dashboard/conversations?${qs}` : '/api/dashboard/conversations')
  },

  getConversationDetail: (threadId: string) =>
    request.get(`/api/dashboard/conversations/${threadId}`),

  getStats: () => request.get('/api/dashboard/stats'),

  getFeedbacks: (params: { rating?: string; agent_id?: string } = {}) => {
    const query = new URLSearchParams()
    if (params.rating && params.rating !== 'all') query.append('rating', params.rating)
    if (params.agent_id) query.append('agent_id', params.agent_id)
    const qs = query.toString()
    return request.get(qs ? `/api/dashboard/feedbacks?${qs}` : '/api/dashboard/feedbacks')
  },

  getUserStats: () => request.get('/api/dashboard/stats/users'),

  getToolStats: () => request.get('/api/dashboard/stats/tools'),

  getKnowledgeStats: () => request.get('/api/dashboard/stats/knowledge'),

  getAgentStats: () => request.get('/api/dashboard/stats/agents'),

  getCallTimeseries: (type = 'agents', timeRange = '14days') =>
    request.get(
      `/api/dashboard/stats/calls/timeseries?type=${encodeURIComponent(type)}&time_range=${encodeURIComponent(timeRange)}`
    ),

  /** 分区并行拉取，单模块失败不影响其他模块 */
  getAllStats: async () => {
    const [basic, users, tools, knowledge, agents] = await Promise.allSettled([
      request.get('/api/dashboard/stats'),
      request.get('/api/dashboard/stats/users'),
      request.get('/api/dashboard/stats/tools'),
      request.get('/api/dashboard/stats/knowledge'),
      request.get('/api/dashboard/stats/agents')
    ])

    return {
      basic: basic.status === 'fulfilled' ? basic.value : null,
      users: users.status === 'fulfilled' ? users.value : null,
      tools: tools.status === 'fulfilled' ? tools.value : null,
      knowledge: knowledge.status === 'fulfilled' ? knowledge.value : null,
      agents: agents.status === 'fulfilled' ? agents.value : null,
      errors: {
        basic: basic.status === 'rejected' ? basic.reason : null,
        users: users.status === 'rejected' ? users.reason : null,
        tools: tools.status === 'rejected' ? tools.reason : null,
        knowledge: knowledge.status === 'rejected' ? knowledge.reason : null,
        agents: agents.status === 'rejected' ? agents.reason : null
      }
    }
  }
}
