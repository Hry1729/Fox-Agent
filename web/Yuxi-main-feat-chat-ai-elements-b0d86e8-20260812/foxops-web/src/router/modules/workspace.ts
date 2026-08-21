import { AppRouteRecord } from '@/types/router'

export const workspaceRoutes: AppRouteRecord = {
  name: 'Workspace',
  path: '/workspace',
  component: '/workspace/index',
  meta: {
    title: '工作区',
    icon: 'ri:folder-6-line',
    keepAlive: true
  }
}
