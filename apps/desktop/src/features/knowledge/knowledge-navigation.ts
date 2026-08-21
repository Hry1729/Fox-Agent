import type { WorkspaceView } from '@/features/workspace/types'

export const YUXI_LOGIN_RETURN_KEY = 'fox:yuxi-login-return'
export const YUXI_SERVICE_RETURN_KEY = 'fox:yuxi-service-return'

export function resolveYuxiLoginReturn(value: string | null): WorkspaceView {
  if (value === 'knowledge' || value === 'agents') return value
  return 'settings-yuxi'
}

export function resolveYuxiServiceReturn(value: string | null): WorkspaceView {
  if (value === 'knowledge' || value === 'login') return value
  return 'settings-yuxi'
}
