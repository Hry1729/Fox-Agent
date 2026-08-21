import type { WorkspaceView } from './types'

export interface WorkspaceReturnRoute {
  view: WorkspaceView
  entityId: string | null
  documentId: string | null
}

export interface ManagementReturnRoutes {
  settings: WorkspaceReturnRoute
  knowledge: WorkspaceReturnRoute
}

export const SETTINGS_WORKSPACE_VIEWS: readonly WorkspaceView[] = [
  'settings',
  'onboarding',
  'settings-ai',
  'settings-models',
  'settings-yuxi',
  'settings-projects',
  'settings-conversation',
  'settings-usage',
  'settings-extensions',
  'settings-about',
  'model-service',
  'service',
  'skills',
  'mcp',
  'maintenance',
  'login',
]

export function managementSection(view: WorkspaceView): 'settings' | 'knowledge' | null {
  if (SETTINGS_WORKSPACE_VIEWS.includes(view)) return 'settings'
  if (view === 'knowledge' || view === 'knowledge-detail' || view === 'knowledge-graph') return 'knowledge'
  return null
}

export function captureManagementReturnRoutes(
  routes: ManagementReturnRoutes,
  current: WorkspaceReturnRoute,
  targetView: WorkspaceView,
): ManagementReturnRoutes {
  if (current.view === 'onboarding' && targetView !== 'onboarding') {
    return { ...routes, settings: current }
  }
  const activeSection = managementSection(current.view)
  const targetSection = managementSection(targetView)
  if (targetSection === 'settings' && activeSection !== 'settings') {
    return { ...routes, settings: current }
  }
  if (targetSection === 'knowledge' && activeSection !== 'knowledge') {
    const previous = activeSection === 'settings' ? routes.settings : current
    return {
      ...routes,
      knowledge: managementSection(previous.view) === 'knowledge' ? routes.knowledge : previous,
    }
  }
  return routes
}

export function managementExitRoute(activeView: WorkspaceView, routes: ManagementReturnRoutes): WorkspaceReturnRoute {
  return managementSection(activeView) === 'settings' ? routes.settings : routes.knowledge
}
