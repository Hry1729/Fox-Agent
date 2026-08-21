import { request } from './request'

/** 已启用/候选模型条目 */
export interface EnabledModel {
  id: string
  display_name?: string
  name?: string
  type?: string
  source?: string
  protocol_override?: string | null
  base_url_override?: string | null
  context_length?: number | null
  dimension?: number | null
  batch_size?: number | null
  supported_parameters?: string[]
  extra?: Record<string, unknown>
  enabled?: boolean
  pricing?: Record<string, unknown>
  input_modalities?: string[]
  architecture?: Record<string, unknown>
  raw_metadata?: Record<string, unknown>
}

/** 模型供应商配置 */
export interface ModelProvider {
  id?: number
  provider_id: string
  display_name: string
  provider_type?: string
  default_protocol?: string | null
  base_url?: string
  embedding_base_url?: string | null
  rerank_base_url?: string | null
  models_endpoint?: string | null
  embedding_models_endpoint?: string | null
  rerank_models_endpoint?: string | null
  api_key_env?: string | null
  api_key?: string | null
  capabilities?: string[]
  enabled_models?: EnabledModel[]
  headers_json?: Record<string, unknown>
  extra_json?: Record<string, unknown>
  is_enabled?: boolean
  is_builtin?: boolean
  credential_status?: string
  created_by?: string
  updated_by?: string
  created_at?: string
  updated_at?: string
}

/** 模型连接测试结果 */
export interface ModelStatusResult {
  spec: string
  status: 'available' | 'unavailable' | 'unsupported' | 'error' | string
  message?: string
}

export type ModelProviderPayload = Partial<{
  provider_id: string
  display_name: string
  provider_type: string
  default_protocol: string | null
  base_url: string
  embedding_base_url: string | null
  rerank_base_url: string | null
  models_endpoint: string | null
  embedding_models_endpoint: string | null
  rerank_models_endpoint: string | null
  api_key_env: string | null
  api_key: string | null
  capabilities: string[]
  enabled_models: EnabledModel[]
  headers_json: Record<string, unknown>
  extra_json: Record<string, unknown>
  is_enabled: boolean
  is_builtin: boolean
}>

/**
 * 模型供应商 API（对应 Yuxi web/apis/system_api.js 的 modelProviderApi）
 */
export const modelProviderApi = {
  getProviders() {
    return request.get<{ success?: boolean; data?: ModelProvider[] } | ModelProvider[]>(
      '/api/system/model-providers'
    )
  },

  getProvider(providerId: string) {
    return request.get<{ success?: boolean; data?: ModelProvider } | ModelProvider>(
      `/api/system/model-providers/${encodeURIComponent(providerId)}`
    )
  },

  getV2Models(modelType = 'chat') {
    return request.get(
      `/api/system/model-providers/models/v2?model_type=${encodeURIComponent(modelType)}`
    )
  },

  refreshModelCache() {
    return request.post('/api/system/model-providers/models/cache/refresh', {})
  },

  getModelStatusBySpec(spec: string) {
    return request.get<{ success?: boolean; data?: ModelStatusResult } | ModelStatusResult>(
      `/api/system/model-providers/models/status?spec=${encodeURIComponent(spec)}`
    )
  },

  createProvider(payload: ModelProviderPayload) {
    return request.post('/api/system/model-providers', payload)
  },

  updateProvider(providerId: string, payload: ModelProviderPayload) {
    return request.put(
      `/api/system/model-providers/${encodeURIComponent(providerId)}`,
      payload
    )
  },

  deleteProvider(providerId: string) {
    return request.del(`/api/system/model-providers/${encodeURIComponent(providerId)}`)
  },

  fetchRemoteModels(providerId: string) {
    return request.get<{ success?: boolean; data?: EnabledModel[] } | EnabledModel[]>(
      `/api/system/model-providers/${encodeURIComponent(providerId)}/remote-models`
    )
  }
}
