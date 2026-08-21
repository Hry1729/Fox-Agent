import { AppRouteRecord } from '@/types/router'

/**
 * 智能体管理：单层侧栏菜单（与「对话」「工作区」一致，不做二级展开）
 */
export const agentManageRoutes: AppRouteRecord = {
  name: 'AgentManage',
  path: '/agent/manage',
  component: '/agent/manage/index',
  meta: {
    title: '智能体管理',
    icon: 'ri:robot-2-line',
    keepAlive: true
  }
}
