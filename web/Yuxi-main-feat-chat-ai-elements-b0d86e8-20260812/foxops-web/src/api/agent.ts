import { request } from './request'
import { useUserStore } from '@/store/modules/user'

/**
 * 智能体 / 对话 / Run 接口（搬自 Yuxi apis/agent_api.js，底层换成 foxops request）
 * 权限：任何已登录用户
 */
export const agentApi = {
  /** 简单聊天调用（非流式） */
  simpleCall: (query: string) => request.post('/api/chat/call', { query }),

  /** 生成对话标题 */
  generateTitle: async (query: string, modelSpec: any) => {
    const response = await request.post<any>('/api/chat/call', {
      query: `根据以下对话内容生成一个简短的标题（最多30个字符，中英文均可），不要包含 markdown 标记：\n\n${query.slice(0, 2000)}`,
      meta: { model_spec: modelSpec }
    })
    return response.response
  },

  /** 获取智能体列表 */
  getAgents: ({ includeSubagents = false } = {}) => {
    const params = new URLSearchParams()
    if (includeSubagents) params.set('include_subagents', 'true')
    const query = params.toString()
    return request.get(query ? `/api/agent?${query}` : '/api/agent')
  },

  getAgentBackends: () => request.get('/api/agent/backends'),

  /** 获取单个智能体详情 */
  getAgentDetail: (agentId: string) => request.get(`/api/agent/${agentId}`),

  /** 获取智能体历史消息 */
  getAgentHistory: (threadId: string) => request.get(`/api/chat/thread/${threadId}/history`),

  /** 获取指定会话的 AgentState */
  getAgentState: (threadId: string, { includeMessages = false } = {}) =>
    request.get(`/api/chat/thread/${threadId}/state${includeMessages ? '?include_messages=true' : ''}`),

  /** 提交消息反馈 */
  submitMessageFeedback: (messageId: number, rating: string, reason: string | null = null) =>
    request.post(`/api/chat/message/${messageId}/feedback`, { rating, reason }),

  getMessageFeedback: (messageId: number) => request.get(`/api/chat/message/${messageId}/feedback`),

  createAgent: (payload: any) => request.post('/api/agent', payload),
  updateAgent: (agentId: string, payload: any) => request.put(`/api/agent/${agentId}`, payload),
  deleteAgent: (agentId: string) => request.del(`/api/agent/${agentId}`),

  /** 创建异步运行任务（Run）—— 发消息的核心 */
  createAgentRun: (data: any) =>
    request.post('/api/agent/runs', {
      query: data.query,
      agent_id: data.agent_id,
      thread_id: data.thread_id,
      meta: data.meta || {},
      image_content: data.image_content || null,
      model_spec: data.model_spec || null,
      resume: data.resume ?? null,
      parent_run_id: data.parent_run_id || null,
      resume_request_id: data.resume_request_id || null
    }),

  /** 获取 Run 状态 */
  getAgentRun: (runId: string) => request.get(`/api/agent/runs/${runId}`),

  /** 取消 Run */
  cancelAgentRun: (runId: string) => request.post(`/api/agent/runs/${runId}/cancel`, {}),

  /** 获取线程活跃 Run */
  getThreadActiveRun: (threadId: string) => request.get(`/api/agent/thread/${threadId}/active_run`),

  /**
   * 打开 Run 事件 SSE 连接（调用方负责关闭）
   * 注意：不走 request 封装，用原生 fetch 拿 Response 供流式读取。
   * 事件带 Redis seq（Last-Event-ID），支持断线续传。
   */
  streamAgentRunEvents: (
    runId: string,
    afterSeq = '0-0',
    options: { signal?: AbortSignal; verbose?: boolean } = {}
  ): Promise<Response> => {
    const { signal, verbose = false } = options
    const headers: Record<string, string> = {
      Authorization: `Bearer ${useUserStore().accessToken}`
    }
    const cursor = String(afterSeq || '0-0')
    if (cursor && cursor !== '0-0') {
      headers['Last-Event-ID'] = cursor
    }
    const params = new URLSearchParams({ verbose: String(verbose) })
    return fetch(`/api/agent/runs/${runId}/events?${params.toString()}`, {
      method: 'GET',
      headers,
      signal
    })
  }
}

/** 多模态图片上传 */
export const multimodalApi = {
  uploadImage: (file: File) => {
    const formData = new FormData()
    formData.append('file', file)
    return request.post('/api/chat/image/upload', formData)
  }
}
