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

export type LocalKnowledgeRouteView = 'home' | 'files' | 'list' | 'detail' | 'documents' | 'import' | 'jobs' | 'models' | 'retrieval' | 'settings'

const localKnowledgeViews: Partial<Record<WorkspaceView, LocalKnowledgeRouteView>> = {
  'local-knowledge-home': 'home',
  'local-files': 'files',
  'local-knowledge': 'list',
  'local-knowledge-detail': 'detail',
  'local-knowledge-documents': 'documents',
  'local-knowledge-import': 'import',
  'local-knowledge-jobs': 'jobs',
  'local-knowledge-models': 'models',
  'local-knowledge-retrieval': 'retrieval',
  'local-knowledge-settings': 'settings',
}

const workspaceViewsForLocalKnowledge: Record<LocalKnowledgeRouteView, WorkspaceView> = {
  home: 'local-knowledge-home',
  files: 'local-files',
  list: 'local-knowledge',
  detail: 'local-knowledge-detail',
  documents: 'local-knowledge-documents',
  import: 'local-knowledge-import',
  jobs: 'local-knowledge-jobs',
  models: 'local-knowledge-models',
  retrieval: 'local-knowledge-retrieval',
  settings: 'local-knowledge-settings',
}

export function localKnowledgeViewForWorkspace(view: WorkspaceView): LocalKnowledgeRouteView | null {
  return localKnowledgeViews[view] ?? null
}

export function workspaceViewForLocalKnowledge(view: LocalKnowledgeRouteView): WorkspaceView {
  return workspaceViewsForLocalKnowledge[view]
}

export function localKnowledgeBackRoute(view: WorkspaceView, knowledgeBaseId?: string | null): { view: WorkspaceView; entityId?: string } {
  if (view === 'local-knowledge-home') return { view: 'chat' }
  if (view === 'local-files') return { view: 'local-knowledge' }
  if (view === 'local-knowledge' || view === 'local-knowledge-models') return { view: 'local-knowledge-home' }
  if (view === 'local-knowledge-detail') return { view: 'local-knowledge' }
  return { view: 'local-knowledge-detail', entityId: knowledgeBaseId ?? undefined }
}

export function remoteKnowledgeBackRoute(view: WorkspaceView, knowledgeBaseId?: string | null): { view: WorkspaceView; entityId?: string } {
  if (view === 'knowledge-graph' && knowledgeBaseId) return { view: 'knowledge-detail', entityId: knowledgeBaseId }
  return { view: 'knowledge' }
}

export const SETTINGS_WORKSPACE_VIEWS: readonly WorkspaceView[] = [
  'settings',
  'onboarding',
  'settings-ai',
  'settings-models',
  'settings-yuxi',
  'settings-projects',
  'settings-conversation',
  'settings-shortcuts',
  'settings-usage',
  'settings-extensions',
  'settings-about',
  'settings-data',
  'model-service',
  'service',
  'skills',
  'mcp',
  'maintenance',
  'login',
]

/**
 * The settings sidebar has seven destinations. Older view ids (deep links,
 * command palette entries, history entries, other pages' "go to" buttons) stay
 * valid and land on the page that now hosts their content, scrolled to the
 * matching section.
 */
export type SettingsSectionAnchor = 'shortcuts' | 'projects' | 'usage' | 'maintenance' | 'about'

export interface SettingsRouteRedirect {
  view: WorkspaceView
  section: SettingsSectionAnchor
}

const legacySettingsRedirects: Partial<Record<WorkspaceView, SettingsRouteRedirect>> = {
  'settings-shortcuts': { view: 'settings-conversation', section: 'shortcuts' },
  'settings-projects': { view: 'settings-ai', section: 'projects' },
  'settings-usage': { view: 'settings-data', section: 'usage' },
  maintenance: { view: 'settings-data', section: 'maintenance' },
  'settings-about': { view: 'settings-data', section: 'about' },
}

export function settingsRouteRedirect(view: WorkspaceView): SettingsRouteRedirect | null {
  return legacySettingsRedirects[view] ?? null
}

/** The sidebar destination that hosts a settings view (sub-pages included). */
export function settingsNavigationView(view: WorkspaceView): WorkspaceView {
  const redirect = settingsRouteRedirect(view)
  if (redirect) return redirect.view
  if (view === 'onboarding') return 'settings'
  if (view === 'model-service') return 'settings-models'
  if (view === 'service' || view === 'login') return 'settings-yuxi'
  if (view === 'skills' || view === 'mcp') return 'settings-extensions'
  return view
}

export function isSettingsSectionAnchor(value: unknown): value is SettingsSectionAnchor {
  return value === 'shortcuts' || value === 'projects' || value === 'usage' || value === 'maintenance' || value === 'about'
}

export function managementSection(view: WorkspaceView): 'settings' | 'knowledge' | null {
  if (SETTINGS_WORKSPACE_VIEWS.includes(view)) return 'settings'
  if (
    view === 'knowledge'
    || view === 'knowledge-detail'
    || view === 'knowledge-graph'
    || view === 'local-knowledge-home'
    || view === 'local-files'
    || view === 'local-knowledge'
    || view === 'local-knowledge-detail'
    || view === 'local-knowledge-documents'
    || view === 'local-knowledge-import'
    || view === 'local-knowledge-jobs'
    || view === 'local-knowledge-models'
    || view === 'local-knowledge-retrieval'
    || view === 'local-knowledge-settings'
  ) return 'knowledge'
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
