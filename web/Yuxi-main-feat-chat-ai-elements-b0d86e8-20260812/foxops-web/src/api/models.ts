import { request } from './request'

/** 模型提供方相关接口（嵌入模型选择） */
export const modelProviderApi = {
  getModelsV2(modelType: string) {
    return request.get(
      `/api/system/model-providers/models/v2?model_type=${encodeURIComponent(modelType)}`
    )
  },

  getModelStatus(spec: string) {
    return request.get(`/api/system/model-providers/models/status?spec=${encodeURIComponent(spec)}`)
  }
}
