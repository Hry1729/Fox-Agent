import { AppRouteRecord } from '@/types/router'

/**
 * 对话页路由（复刻 Yuxi AgentView）
 * 单层路由，/chat 直接是对话页。
 */
export const chatRoutes: AppRouteRecord = {
  name: 'Chat',
  path: '/chat',
  component: '/chat/agent',
  meta: {
    title: '对话',
    icon: 'ri:chat-3-line',
    keepAlive: true
  }
}

