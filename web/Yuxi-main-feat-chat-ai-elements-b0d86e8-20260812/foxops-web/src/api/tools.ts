import { request } from './request'

export interface SystemToolArg {
  name: string
  type?: string
  description?: string
  required?: boolean
}

export interface SystemTool {
  id?: string
  slug?: string
  name: string
  description?: string
  category?: string
  tags?: string[]
  config_guide?: string
  args?: SystemToolArg[]
}

export const toolsApi = {
  getTools(category?: string | null) {
    const query = category ? `?${new URLSearchParams({ category }).toString()}` : ''
    return request.get<{ data?: SystemTool[] } | SystemTool[]>(`/api/system/tools${query}`)
  },

  getToolOptions() {
    return request.get('/api/system/tools/options')
  }
}
