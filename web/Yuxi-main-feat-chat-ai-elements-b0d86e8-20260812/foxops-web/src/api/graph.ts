import { request } from './request'

const buildQuery = (params: Record<string, unknown> = {}) => {
  const query = new URLSearchParams()
  Object.entries(params).forEach(([key, value]) => {
    if (value !== undefined && value !== null && value !== '') query.set(key, String(value))
  })
  const text = query.toString()
  return text ? `?${text}` : ''
}

export const graphApi = {
  getGraphs() {
    return request.get('/api/graph/list')
  },

  getSubgraph(
    kbId: string,
    params: {
      nodeLabel?: string
      maxDepth?: number
      maxNodes?: number
      excludeChunk?: boolean
    } = {}
  ) {
    return request.get(
      `/api/graph/subgraph${buildQuery({
        kb_id: kbId,
        node_label: params.nodeLabel ?? '*',
        max_depth: params.maxDepth ?? 2,
        max_nodes: params.maxNodes ?? 100,
        exclude_chunk: params.excludeChunk ?? false
      })}`
    )
  },

  getGraphStats(kbId: string) {
    return request.get(`/api/graph/stats${buildQuery({ kb_id: kbId })}`)
  },

  getGraphLabels(kbId: string) {
    return request.get(`/api/graph/labels${buildQuery({ kb_id: kbId })}`)
  },

  getGraphBuildStatus(kbId: string) {
    return request.get(`/api/knowledge/databases/${encodeURIComponent(kbId)}/graph-build/status`)
  },

  configureGraphBuild(kbId: string, data: Record<string, unknown>) {
    return request.post(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/graph-build/config`,
      data
    )
  },

  startGraphIndex(kbId: string, batchSize = 20) {
    return request.post(`/api/knowledge/databases/${encodeURIComponent(kbId)}/graph-build/index`, {
      batch_size: batchSize
    })
  },

  resetGraphBuild(kbId: string, data: Record<string, unknown> = {}) {
    return request.post(
      `/api/knowledge/databases/${encodeURIComponent(kbId)}/graph-build/reset`,
      data
    )
  }
}
