import { request } from './request'

/**
 * Yuxi 登录返回（POST /api/auth/token，OAuth2PasswordRequestForm）
 * 支持用 uid 或手机号作为登录标识。
 */
export interface YuxiLoginResult {
  access_token: string
  token_type: string
  user_id: number
  username: string
  uid: string
  phone_number: string | null
  avatar: string | null
  role: string
  department_id: number | null
  department_name: string | null
}

/**
 * Yuxi 当前用户信息（GET /api/auth/me）
 */
export interface YuxiUserInfo {
  id: number
  username: string
  uid: string
  phone_number: string | null
  avatar: string | null
  role: string
  department_id: number | null
  department_name: string | null
}

/**
 * 登录（支持 uid 或手机号）
 * @param loginId 用户标识（uid 或手机号）
 * @param password 密码
 */
export function fetchLogin(loginId: string, password: string): Promise<YuxiLoginResult> {
  const formData = new FormData()
  formData.append('username', loginId)
  formData.append('password', password)
  return request.post<YuxiLoginResult>('/api/auth/token', formData)
}

/**
 * Yuxi 角色 -> Art Design Pro roles 映射
 */
function mapYuxiRoles(role: string): string[] {
  if (role === 'superadmin') return ['R_SUPER', 'R_ADMIN']
  if (role === 'admin') return ['R_ADMIN']
  return []
}

/**
 * Yuxi 用户信息 -> Art Design Pro UserInfo 适配器
 */
function toUserInfo(yuxi: YuxiUserInfo): Api.Auth.UserInfo {
  return {
    userId: yuxi.id,
    uid: yuxi.uid,
    role: yuxi.role,
    userName: yuxi.username,
    avatar: yuxi.avatar || '',
    email: '',
    roles: mapYuxiRoles(yuxi.role),
    buttons: []
  }
}

/**
 * 获取当前用户信息（已适配为 Art Design Pro UserInfo）
 */
export async function fetchGetUserInfo(): Promise<Api.Auth.UserInfo> {
  const yuxi = await request.get<YuxiUserInfo>('/api/auth/me')
  return toUserInfo(yuxi)
}
