/**
 * 解包 Yuxi 常见响应外壳：{ success, data } / { message, data } / 直接数组或对象
 */
export function unwrapApiData<T = any>(payload: any, fallback: T | null = null): T {
  if (payload == null) return (fallback as T) ?? (null as T)
  if (Array.isArray(payload)) return payload as T
  if (typeof payload !== 'object') return payload as T
  if ('data' in payload && payload.data !== undefined) return payload.data as T
  if ('servers' in payload && payload.servers !== undefined) return payload.servers as T
  if ('skills' in payload && payload.skills !== undefined) return payload.skills as T
  if ('databases' in payload && payload.databases !== undefined) return payload.databases as T
  if ('tools' in payload && payload.tools !== undefined) return payload.tools as T
  if ('agents' in payload && payload.agents !== undefined) return payload.agents as T
  if ('providers' in payload && payload.providers !== undefined) return payload.providers as T
  if ('items' in payload && payload.items !== undefined) return payload.items as T
  return payload as T
}

export function unwrapList<T = any>(payload: any): T[] {
  const data: any = unwrapApiData<any>(payload, [])
  if (Array.isArray(data)) return data
  if (data && typeof data === 'object') {
    if (Array.isArray(data.items)) return data.items
    if (Array.isArray(data.agents)) return data.agents
    if (Array.isArray(data.providers)) return data.providers
    if (Array.isArray(data.runs)) return data.runs
    if (Array.isArray(data.datasets)) return data.datasets
  }
  return []
}
