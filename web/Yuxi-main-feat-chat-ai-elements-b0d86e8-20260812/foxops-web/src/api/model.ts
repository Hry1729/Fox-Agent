import { request } from './request'

/**
 * 模型提供商接口（搬自 Yuxi apis/system_api.js 的 modelProviderApi 部分）
 */
export const modelProviderApi = {
  /** 获取 v2 模型列表（按 provider 分组） */
  getV2Models: (modelType = 'chat') =>
    request.get(`/api/system/model-providers/models/v2?model_type=${modelType}`),

  /** 刷新模型缓存 */
  refreshModelCache: () =>
    request.post('/api/system/model-providers/models/cache/refresh', {}),

  /** 获取指定 spec 的模型状态 */
  getModelStatusBySpec: (spec: string) =>
    request.get(`/api/system/model-providers/models/status?spec=${encodeURIComponent(spec)}`)
}
