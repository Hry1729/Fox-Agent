import { request } from './request'

export type McpTransport = 'streamable_http' | 'sse' | 'stdio'

export interface McpServer {
  slug: string
  name: string
  description?: string
  icon?: string
  transport?: McpTransport | string
  enabled?: boolean
  created_by?: string
  tags?: string[]
  url?: string
  headers?: Record<string, string> | string
  http_timeout?: number
  sse_read_timeout?: number
  command?: string
  args?: string[]
  env?: Record<string, string>
  created_at?: string
  updated_at?: string
}

export interface McpToolItem {
  name: string
  description?: string
  enabled?: boolean
  inputSchema?: Record<string, unknown>
  input_schema?: Record<string, unknown>
}

export const mcpApi = {
  getServers() {
    return request.get<{ servers?: McpServer[] } | McpServer[]>('/api/system/mcp-servers')
  },

  getServer(slug: string) {
    return request.get<McpServer>(`/api/system/mcp-servers/${encodeURIComponent(slug)}`)
  },

  createServer(data: Record<string, unknown>) {
    return request.post('/api/system/mcp-servers', data)
  },

  updateServer(slug: string, data: Record<string, unknown>) {
    return request.put(`/api/system/mcp-servers/${encodeURIComponent(slug)}`, data)
  },

  deleteServer(slug: string) {
    return request.del(`/api/system/mcp-servers/${encodeURIComponent(slug)}`)
  },

  testServer(slug: string) {
    return request.post(`/api/system/mcp-servers/${encodeURIComponent(slug)}/test`, {})
  },

  updateServerStatus(slug: string, enabled: boolean) {
    return request.put(`/api/system/mcp-servers/${encodeURIComponent(slug)}/status`, { enabled })
  },

  getServerTools(slug: string) {
    return request.get<{ tools?: McpToolItem[] } | McpToolItem[]>(
      `/api/system/mcp-servers/${encodeURIComponent(slug)}/tools`
    )
  },

  refreshServerTools(slug: string) {
    return request.post(`/api/system/mcp-servers/${encodeURIComponent(slug)}/tools/refresh`, {})
  },

  toggleServerTool(slug: string, toolName: string) {
    return request.put(
      `/api/system/mcp-servers/${encodeURIComponent(slug)}/tools/${encodeURIComponent(toolName)}/toggle`,
      {}
    )
  }
}
