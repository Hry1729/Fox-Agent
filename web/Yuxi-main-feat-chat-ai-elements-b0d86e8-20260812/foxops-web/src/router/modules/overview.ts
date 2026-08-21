import { AppRouteRecord } from '@/types/router'

/**
 * 数据总览：单层菜单（与对话一致），仅超级管理员可见
 * 路径用 /overview，避免与演示仪表盘 /dashboard/* 冲突
 */
export const overviewRoutes: AppRouteRecord = {
  name: 'DataOverview',
  path: '/overview',
  component: '/overview/index',
  meta: {
    title: '数据总览',
    icon: 'ri:bar-chart-2-line',
    keepAlive: false,
    roles: ['R_SUPER']
  }
}
