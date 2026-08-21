import { AppRouteRecord } from '@/types/router'
import { chatRoutes } from './chat'
import { workspaceRoutes } from './workspace'
import { agentManageRoutes } from './agent-manage'
import { extensionsRoutes } from './extensions'
import { overviewRoutes } from './overview'
import { systemRoutes } from './system'
import { safeguardRoutes } from './safeguard'

/**
 * 导出所有模块化路由
 */
export const routeModules: AppRouteRecord[] = [
  chatRoutes,
  workspaceRoutes,
  agentManageRoutes,
  extensionsRoutes,
  overviewRoutes,
  systemRoutes,
  safeguardRoutes
]
