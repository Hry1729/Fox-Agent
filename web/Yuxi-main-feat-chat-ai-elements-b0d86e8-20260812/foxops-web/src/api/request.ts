/**
 * FoxOps 请求封装（适配 Yuxi 后端）
 *
 * Yuxi 后端是 FastAPI 风格：
 * - 成功：HTTP 2xx，响应体直接是业务数据（无 {code,msg,data} 外壳）
 * - 失败：HTTP 4xx/5xx，响应体 { detail: string | object }
 * - 鉴权：Authorization: Bearer <access_token>
 *
 * 与模板自带的 @/utils/http（假设 {code,msg,data} 外壳）不同，
 * FoxOps 接口统一用本封装；模板原封装保留不动，供模板自带页面参考。
 */
import axios, { type AxiosRequestConfig, type InternalAxiosRequestConfig } from 'axios'
import { ElMessage } from 'element-plus'
import { useUserStore } from '@/store/modules/user'

const service = axios.create({
  baseURL: import.meta.env.VITE_API_URL || '/',
  timeout: 15000
})

// 请求拦截器：注入 JWT（Yuxi 用 Bearer <access_token>）
service.interceptors.request.use(
  (config: InternalAxiosRequestConfig) => {
    const { accessToken } = useUserStore()
    if (accessToken) {
      config.headers.set('Authorization', `Bearer ${accessToken}`)
    }
    return config
  },
  (error) => Promise.reject(error)
)

/** 解析 Yuxi 后端错误文案（detail 可能是字符串，也可能是 { message, error } 对象） */
function resolveErrorMessage(data: any): string {
  if (!data) return '请求失败'
  if (typeof data.detail === 'string') return data.detail
  if (data.detail && typeof data.detail === 'object') {
    return data.detail.message || data.detail.error || '请求失败'
  }
  return data.message || '请求失败'
}

// 响应拦截器：成功直接返回业务数据，失败统一提示
service.interceptors.response.use(
  (response) => response.data,
  (error) => {
    const status = error.response?.status
    const message = resolveErrorMessage(error.response?.data)
    if (error instanceof Error) error.message = message

    if (status === 401) {
      ElMessage.error('登录已过期，请重新登录')
      useUserStore().logOut()
    } else if (status === 403) {
      ElMessage.error('没有权限执行此操作')
    } else if (status && status >= 500) {
      ElMessage.error('服务器内部错误，请查看后端日志')
    } else if (status) {
      ElMessage.error(message)
    } else {
      ElMessage.error('网络异常，请检查连接')
    }
    return Promise.reject(error)
  }
)

export default service

/** 便捷方法：返回值直接是业务数据 */
export const request = {
  get<T = any>(url: string, config?: AxiosRequestConfig): Promise<T> {
    return service.get<T, T>(url, config)
  },
  post<T = any>(url: string, data?: any, config?: AxiosRequestConfig): Promise<T> {
    return service.post<T, T>(url, data, config)
  },
  put<T = any>(url: string, data?: any, config?: AxiosRequestConfig): Promise<T> {
    return service.put<T, T>(url, data, config)
  },
  del<T = any>(url: string, config?: AxiosRequestConfig): Promise<T> {
    return service.delete<T, T>(url, config)
  }
}
