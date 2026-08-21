import { request } from './request'
import { authenticatedBlobRequest } from './workspace'

export interface AccessibleKnowledgeBase {
  id?: string
  kb_id: string
  name: string
  created_by?: string
  supports_documents?: boolean
  kb_type?: string
  description?: string
  file_count?: number
}

export interface KnowledgeDatabase extends AccessibleKnowledgeBase {
  embed_model?: string
  embedding_model_spec?: string
  chunk_size?: number
  share_config?: Record<string, unknown>
  additional_params?: Record<string, unknown>
  auto_generate_questions?: boolean
  created_at?: string
  row_count?: number
  [key: string]: unknown
}

export interface KnowledgeTypeInfo {
  label?: string
  description?: string
  requires_embedding_model?: boolean
  supports_documents?: boolean
  create_params?: Array<Record<string, unknown>>
  [key: string]: unknown
}

const buildQuery = (params: Record<string, unknown> = {}) => {
  const query = new URLSearchParams()
  Object.entries(params).forEach(([key, value]) => {
    if (value !== undefined && value !== null && value !== '') query.set(key, String(value))
  })
  const text = query.toString()
  return text ? `?${text}` : ''
}

export const knowledgeApi = {
  getAccessibleDatabases() {
    return request.get<{ databases?: AccessibleKnowledgeBase[] }>(
      '/api/knowledge/databases/accessible'
    )
  },

  getDatabases() {
    return request.get<{ databases?: KnowledgeDatabase[] }>('/api/knowledge/databases')
  },

  getDatabaseInfo(kbId: string) {
    return request.get<KnowledgeDatabase>(`/api/knowledge/databases/${encodeURIComponent(kbId)}`)
  },

  createDatabase(payload: Record<string, unknown>) {
    return request.post('/api/knowledge/databases', payload)
  },

  updateDatabase(kbId: string, payload: Record<string, unknown>) {
    return request.put(`/api/knowledge/databases/${encodeURIComponent(kbId)}`, payload)
  },

  deleteDatabase(kbId: string) {
    return request.del(`/api/knowledge/databases/${encodeURIComponent(kbId)}`)
  },

  repairDatabaseStats(kbId: string) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/stats/repair`, {})
  },

  generateDescription(name: string, currentDescription = '', fileList: unknown[] = []) {
    return request.post('/api/knowledge/generate-description', {
      name,
      current_description: currentDescription,
      file_list: fileList
    })
  },

  getTypes() {
    return request.get<
      Record<string, KnowledgeTypeInfo> | { types?: Record<string, KnowledgeTypeInfo> }
    >('/api/knowledge/types')
  },

  getStatistics() {
    return request.get('/api/knowledge/stats')
  },

  listDocuments(kbId: string, params: Record<string, unknown> = {}) {
    return request.get(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents${buildQuery(params)}`
    )
  },

  documentExists(kbId: string, filename: string) {
    return request.get(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/exists${buildQuery({ filename })}`
    )
  },

  createFolder(kbId: string, folderName: string, parentId: string | null = null) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/folders`, {
      folder_name: folderName,
      parent_id: parentId
    })
  },

  addDocuments(kbId: string, items: unknown[], params: Record<string, unknown> = {}) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/documents`, {
      items,
      params
    })
  },

  addUploadedDocuments(kbId: string, items: unknown[], params: Record<string, unknown> = {}) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/add`, {
      items,
      params
    })
  },

  getDocumentInfo(kbId: string, docId: string) {
    return request.get(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/${encodeURIComponent(docId)}`
    )
  },

  getDocumentBasicInfo(kbId: string, docId: string) {
    return request.get(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/${encodeURIComponent(docId)}/basic`
    )
  },

  getDocumentContent(kbId: string, docId: string) {
    return request.get(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/${encodeURIComponent(docId)}/content`
    )
  },

  deleteDocument(kbId: string, fileId: string) {
    return request.del(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/${encodeURIComponent(fileId)}`
    )
  },

  batchDeleteDocuments(kbId: string, fileIds: string[]) {
    return request.del(`/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/batch`, {
      data: fileIds
    })
  },

  downloadDocument(kbId: string, fileId: string) {
    return authenticatedBlobRequest(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/${encodeURIComponent(fileId)}/download`
    )
  },

  parseDocuments(kbId: string, fileIds: string[]) {
    return request.post(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/parse`,
      fileIds
    )
  },

  parsePendingDocuments(kbId: string) {
    return request.post(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/parse-pending`,
      {}
    )
  },

  indexDocuments(kbId: string, fileIds: string[], params: Record<string, unknown> = {}) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/index`, {
      file_ids: fileIds,
      params
    })
  },

  indexPendingDocuments(kbId: string, params: Record<string, unknown> = {}) {
    return request.post(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/documents/index-pending`,
      { params }
    )
  },

  query(kbId: string, query: string, meta: Record<string, unknown> = {}) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/query`, {
      query,
      meta
    })
  },

  queryTest(kbId: string, query: string, meta: Record<string, unknown> = {}) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/query-test`, {
      query,
      meta
    })
  },

  getQueryParams(kbId: string) {
    return request.get(`/api/knowledge/databases/${encodeURIComponent(kbId)}/query-params`)
  },

  updateQueryParams(kbId: string, payload: Record<string, unknown>) {
    return request.put(`/api/knowledge/databases/${encodeURIComponent(kbId)}/query-params`, payload)
  },

  generateSampleQuestions(kbId: string, count = 10) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/sample-questions`, {
      count
    })
  },

  getSampleQuestions(kbId: string) {
    return request.get(`/api/knowledge/databases/${encodeURIComponent(kbId)}/sample-questions`)
  },

  fetchUrl(url: string, kbId?: string | null) {
    return request.post('/api/knowledge/files/fetch-url', { url, kb_id: kbId })
  },

  importWorkspaceFiles(kbId: string, paths: string[]) {
    return request.post('/api/knowledge/files/import-workspace', { kb_id: kbId, paths })
  },

  uploadFile(file: File, kbId?: string | null) {
    const formData = new FormData()
    formData.append('file', file)
    const url = kbId
      ? `/api/knowledge/files/upload?kb_id=${encodeURIComponent(kbId)}`
      : '/api/knowledge/files/upload'
    return request.post(url, formData)
  },

  getSupportedFileTypes() {
    return request.get('/api/knowledge/files/supported-types')
  },

  uploadFolder(file: File, kbId: string) {
    const formData = new FormData()
    formData.append('file', file)
    return request.post(
      `/api/knowledge/files/upload-folder?kb_id=${encodeURIComponent(kbId)}`,
      formData
    )
  },

  processFolder(payload: { file_path: string; kb_id: string; content_hash?: string }) {
    return request.post('/api/knowledge/files/process-folder', payload)
  },

  getMindmapDatabases() {
    return request.get('/api/knowledge/mindmap/databases')
  },

  getMindmapFiles(kbId: string) {
    return request.get(`/api/knowledge/databases/${encodeURIComponent(kbId)}/mindmap/files`)
  },

  generateMindmap(kbId: string, fileIds: string[] = [], userPrompt = '', incremental = false) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/mindmap/generate`, {
      file_ids: fileIds,
      user_prompt: userPrompt,
      incremental
    })
  },

  getMindmap(kbId: string) {
    return request.get(`/api/knowledge/databases/${encodeURIComponent(kbId)}/mindmap`)
  },

  getMindmapDiff(kbId: string) {
    return request.get(`/api/knowledge/databases/${encodeURIComponent(kbId)}/mindmap/diff`)
  }
}

export const evaluationApi = {
  uploadDataset(kbId: string, file: File, metadata: Record<string, string> = {}) {
    const formData = new FormData()
    formData.append('file', file)
    formData.append('name', metadata.name || '')
    formData.append('description', metadata.description || '')
    return request.post(
      `/api/evaluation/databases/${encodeURIComponent(kbId)}/datasets/upload`,
      formData
    )
  },

  listDatasets(kbId: string) {
    return request.get(`/api/evaluation/databases/${encodeURIComponent(kbId)}/datasets`)
  },

  getDataset(kbId: string, datasetId: string, page = 1, pageSize = 50) {
    return request.get(
      `/api/evaluation/databases/${encodeURIComponent(kbId)}/datasets/${encodeURIComponent(datasetId)}${buildQuery({ page, page_size: pageSize })}`
    )
  },

  deleteDataset(datasetId: string) {
    return request.del(`/api/evaluation/datasets/${encodeURIComponent(datasetId)}`)
  },

  downloadDataset(datasetId: string) {
    return authenticatedBlobRequest(
      `/api/evaluation/datasets/${encodeURIComponent(datasetId)}/download`
    )
  },

  generateDataset(kbId: string, params: Record<string, unknown>) {
    return request.post(
      `/api/evaluation/databases/${encodeURIComponent(kbId)}/datasets/generate`,
      params
    )
  },

  runEvaluation(kbId: string, params: Record<string, unknown>) {
    return request.post(`/api/evaluation/databases/${encodeURIComponent(kbId)}/runs`, params)
  },

  listRuns(kbId: string) {
    return request.get(`/api/evaluation/databases/${encodeURIComponent(kbId)}/runs`)
  },

  getRunResults(kbId: string, runId: string, params: Record<string, unknown> = {}) {
    return request.get(
      `/api/evaluation/databases/${encodeURIComponent(kbId)}/runs/${encodeURIComponent(runId)}${buildQuery(
        {
          page: params.page,
          page_size: params.pageSize,
          error_only: params.errorOnly
        }
      )}`
    )
  },

  deleteRun(kbId: string, runId: string) {
    return request.del(
      `/api/evaluation/databases/${encodeURIComponent(kbId)}/runs/${encodeURIComponent(runId)}`
    )
  }
}
