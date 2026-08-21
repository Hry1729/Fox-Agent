import { request } from './request'

/**
 * 会话线程接口（搬自 Yuxi agent_api.js 的 threadApi）
 * MVP 只搬会话 CRUD；文件/附件相关后置。
 */
export const threadApi = {
  /** 获取对话线程列表 */
  getThreads: (agentId: string | null = null, limit = 100, offset = 0) => {
    const params = new URLSearchParams({ limit: String(limit), offset: String(offset) })
    if (agentId) params.set('agent_id', agentId)
    return request.get(`/api/chat/threads?${params.toString()}`)
  },

  /** 创建新对话线程 */
  createThread: (agentId: string, title: string, metadata?: any) =>
    request.post('/api/chat/thread', {
      agent_id: agentId,
      title: title || '新的对话',
      metadata: metadata || {}
    }),

  /** 更新对话线程（标题 / 置顶） */
  updateThread: (threadId: string, title: string | null, isPinned?: boolean) =>
    request.put(`/api/chat/thread/${threadId}`, { title, is_pinned: isPinned }),

  /** 删除对话线程 */
  deleteThread: (threadId: string) => request.del(`/api/chat/thread/${threadId}`),

  // ==================== 附件相关 ====================

  /** 上传临时附件（返回 tmp_file_id / object_name / parse_methods 等） */
  uploadTmpAttachment: (file: File) => {
    const formData = new FormData()
    formData.append('file', file)
    return request.post('/api/chat/attachments/tmp', formData)
  },

  /** 解析临时附件（PDF/图片 -> Markdown） */
  parseTmpAttachment: (payload: any) =>
    request.post('/api/chat/attachments/tmp/parse', payload),

  /** 确认添加临时附件到线程 */
  confirmTmpThreadAttachments: (threadId: string, attachments: any[]) =>
    request.post(`/api/chat/thread/${threadId}/attachments/confirm`, { attachments }),

  /** 获取线程附件列表 */
  getThreadAttachments: (threadId: string) =>
    request.get(`/api/chat/thread/${threadId}/attachments`),

  /** 删除线程附件 */
  deleteThreadAttachment: (threadId: string, fileId: string) =>
    request.del(`/api/chat/thread/${threadId}/attachments/${fileId}`),

  // ==================== 文件工作区 ====================

  /** 列出线程文件（文件树） */
  listThreadFiles: (threadId: string, path = '/home/gem/user-data', recursive = false) =>
    request.get(
      `/api/chat/thread/${threadId}/files?path=${encodeURIComponent(path)}&recursive=${recursive}`
    ),

  /** 读取线程文本文件内容（分页） */
  readThreadFile: (threadId: string, path: string, offset = 0, limit = 2000) =>
    request.get(
      `/api/chat/thread/${threadId}/files/content?path=${encodeURIComponent(path)}&offset=${offset}&limit=${limit}`
    ),

  /** 获取线程文件下载/预览 URL */
  getThreadArtifactUrl: (threadId: string, path: string, download = false) => {
    const encodedPath = path
      .split('/')
      .filter(Boolean)
      .map((segment) => encodeURIComponent(segment))
      .join('/')
    const query = download ? '?download=true' : ''
    return `/api/chat/thread/${threadId}/artifacts/${encodedPath}${query}`
  },

  /** 下载线程文件（带鉴权，返回 Blob） */
  downloadThreadArtifact: async (threadId: string, path: string) => {
    const url = `/api/chat/thread/${threadId}/artifacts/${path
      .split('/')
      .filter(Boolean)
      .map((s) => encodeURIComponent(s))
      .join('/')}?download=true`
    const { useUserStore } = await import('@/store/modules/user')
    const resp = await fetch(url, {
      headers: { Authorization: `Bearer ${useUserStore().accessToken}` }
    })
    if (!resp.ok) throw new Error(`下载失败: ${resp.status}`)
    return resp.blob()
  },

  /** 保存交付物到 workspace/saved_artifacts */
  saveThreadArtifactToWorkspace: (threadId: string, path: string) =>
    request.post(`/api/chat/thread/${threadId}/artifacts/save`, { path })
}
