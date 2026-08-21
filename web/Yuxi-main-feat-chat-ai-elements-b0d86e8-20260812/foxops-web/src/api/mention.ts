import type { AxiosRequestConfig } from 'axios'
import { request } from './request'

/**
 * 提及资源搜索（仅文件类）
 * 后端：GET /api/mention/search?thread_id=...&query=...
 * 返回：[{ name, path, is_dir, source }]
 */
export function searchMentionFiles(threadId: string, query: string, signal?: AbortSignal) {
  const params = new URLSearchParams()
  if (threadId) params.set('thread_id', threadId)
  if (query) params.set('query', query)
  const config: AxiosRequestConfig = signal ? { signal } : {}
  return request.get(`/api/mention/search?${params.toString()}`, config)
}
