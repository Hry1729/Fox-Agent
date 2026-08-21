import { request } from './request'
import { authenticatedBlobRequest } from './workspace'

export interface SkillItem {
  slug: string
  name: string
  description?: string
  enabled?: boolean
  is_builtin?: boolean
  can_manage?: boolean
  source?: string
  source_type?: string
  share_config?: Record<string, unknown>
  tags?: string[]
}

export const skillsApi = {
  listSkills() {
    return request.get<{ skills?: SkillItem[] } | SkillItem[]>('/api/system/skills')
  },

  listAccessibleSkills() {
    return request.get<{ skills?: SkillItem[] } | SkillItem[]>('/api/skills/accessible')
  },

  prepareUpload(file: File) {
    const formData = new FormData()
    formData.append('file', file)
    return request.post('/api/skills/import/prepare', formData)
  },

  listRemoteSkills(source: string) {
    return request.post('/api/skills/remote/list', { source })
  },

  prepareRemoteSkills(payload: Record<string, unknown>) {
    return request.post('/api/skills/remote/prepare', payload)
  },

  searchRemoteSkills(query: string) {
    return request.post('/api/skills/remote/search', { query })
  },

  confirmInstallDraft(draftId: string, shareConfig: Record<string, unknown>) {
    return request.post(`/api/skills/install-drafts/${encodeURIComponent(draftId)}/confirm`, {
      share_config: shareConfig
    })
  },

  discardInstallDraft(draftId: string) {
    return request.del(`/api/skills/install-drafts/${encodeURIComponent(draftId)}`)
  },

  getDependencyOptions(slug?: string) {
    const query = slug ? `?slug=${encodeURIComponent(slug)}` : ''
    return request.get(`/api/system/skills/dependency-options${query}`)
  },

  listBuiltinSkills() {
    return request.get('/api/system/skills/builtin')
  },

  syncBuiltinSkills() {
    return request.post('/api/system/skills/builtin/sync', {})
  },

  getTree(slug: string) {
    return request.get(`/api/system/skills/${encodeURIComponent(slug)}/tree`)
  },

  getFile(slug: string, path: string) {
    return request.get(
      `/api/system/skills/${encodeURIComponent(slug)}/file?path=${encodeURIComponent(path)}`
    )
  },

  createFile(slug: string, payload: Record<string, unknown>) {
    return request.post(`/api/system/skills/${encodeURIComponent(slug)}/file`, payload)
  },

  updateFile(slug: string, payload: Record<string, unknown>) {
    return request.put(`/api/system/skills/${encodeURIComponent(slug)}/file`, payload)
  },

  deleteFile(slug: string, path: string) {
    return request.del(
      `/api/system/skills/${encodeURIComponent(slug)}/file?path=${encodeURIComponent(path)}`
    )
  },

  updateDependencies(slug: string, payload: Record<string, unknown>) {
    return request.put(`/api/system/skills/${encodeURIComponent(slug)}/dependencies`, payload)
  },

  updateShareConfig(slug: string, shareConfig: Record<string, unknown>) {
    return request.put(`/api/system/skills/${encodeURIComponent(slug)}/share-config`, {
      share_config: shareConfig
    })
  },

  updateEnabled(slug: string, enabled: boolean) {
    return request.put(`/api/system/skills/${encodeURIComponent(slug)}/enabled`, { enabled })
  },

  exportSkill(slug: string) {
    return authenticatedBlobRequest(`/api/system/skills/${encodeURIComponent(slug)}/export`)
  },

  deleteSkill(slug: string) {
    return request.del(`/api/system/skills/${encodeURIComponent(slug)}`)
  },

  deleteBatch(slugs: string[]) {
    return request.post('/api/system/skills/delete-batch', { slugs })
  }
}
