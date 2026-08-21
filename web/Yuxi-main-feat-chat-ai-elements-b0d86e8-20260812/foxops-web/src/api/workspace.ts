import { request } from './request'
import { useUserStore } from '@/store/modules/user'
import { buildQuery } from '@/utils/workspace'

export interface WorkspaceEntry {
  name: string
  path: string
  is_dir: boolean
  size?: number | null
  modified_at?: string | null
  source?: 'personal' | 'knowledge'
  file_id?: string | null
  kb_id?: string | null
  path_prefix?: string
  is_virtual_folder?: boolean
}

export interface WorkspaceTreeResponse {
  entries: WorkspaceEntry[]
  path?: string
}

export interface KnowledgeTreeResponse extends WorkspaceTreeResponse {
  parent_id?: string | null
  path_prefix?: string
  page?: number
  page_size?: number
  total?: number
  has_more?: boolean
}

async function resolveBlobError(response: Response): Promise<string> {
  try {
    const data = await response.clone().json()
    if (typeof data?.detail === 'string') return data.detail
    return data?.detail?.message || data?.detail?.error || data?.message || '请求失败'
  } catch {
    return response.statusText || '请求失败'
  }
}

export async function authenticatedBlobRequest(url: string): Promise<Response> {
  const baseUrl = import.meta.env.VITE_API_URL || '/'
  const target = new URL(url, new URL(baseUrl, window.location.origin)).toString()
  const response = await fetch(target, {
    headers: { Authorization: `Bearer ${useUserStore().accessToken}` }
  })
  if (!response.ok) throw new Error(await resolveBlobError(response))
  return response
}

export const workspaceApi = {
  getTree(path = '/', recursive = false, filesOnly = false) {
    const query = buildQuery({ path, recursive, files_only: filesOnly })
    return request.get<WorkspaceTreeResponse>(`/api/workspace/tree?${query}`)
  },

  getFile(path: string) {
    return authenticatedBlobRequest(`/api/workspace/file?${buildQuery({ path })}`)
  },

  saveFile(path: string, content: string) {
    return request.put<{ entry?: WorkspaceEntry }>('/api/workspace/file', { path, content })
  },

  deletePath(path: string) {
    return request.del(`/api/workspace/file?${buildQuery({ path })}`)
  },

  createDirectory(parentPath: string, name: string) {
    return request.post('/api/workspace/directory', { parent_path: parentPath, name })
  },

  uploadFiles(parentPath: string, files: File[]) {
    const formData = new FormData()
    formData.append('parent_path', parentPath)
    files.forEach((file) => formData.append('files', file))
    return request.post('/api/workspace/upload', formData)
  },

  downloadFile(path: string) {
    return authenticatedBlobRequest(`/api/workspace/download?${buildQuery({ path })}`)
  },

  getKnowledgeTree(
    kbId: string,
    params: {
      parentId?: string | null
      pathPrefix?: string
      page?: number
      pageSize?: number
      recursive?: boolean
      filesOnly?: boolean
    } = {}
  ) {
    const query = buildQuery({
      kb_id: kbId,
      parent_id: params.parentId,
      path_prefix: params.pathPrefix,
      page: params.page,
      page_size: params.pageSize,
      recursive: params.recursive ?? false,
      files_only: params.filesOnly ?? false
    })
    return request.get<KnowledgeTreeResponse>(`/api/workspace/knowledge/tree?${query}`)
  },

  getKnowledgeFile(kbId: string, fileId: string, variant = 'parsed') {
    const query = buildQuery({ kb_id: kbId, file_id: fileId, variant })
    return authenticatedBlobRequest(`/api/workspace/knowledge/file?${query}`)
  },

  downloadKnowledgeFile(kbId: string, fileId: string, variant = 'original') {
    const query = buildQuery({ kb_id: kbId, file_id: fileId, variant })
    return authenticatedBlobRequest(`/api/workspace/knowledge/download?${query}`)
  }
}
