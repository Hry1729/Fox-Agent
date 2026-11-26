import { lazy, Suspense, type ReactNode } from 'react'
import type { WorkspaceSchedules } from '@/features/companions/tasks'
import { isCompanionWorkspaceView } from './types'
import type { KnowledgeBaseRecord } from '@/features/conversations/model/types'
import type { LocalKnowledgeGateway, LocalKnowledgeView } from '@/features/local-knowledge'
import type { PluginCardView, PluginGateway } from '@/features/plugins'
import { isPluginSection } from '@/features/plugins/plugin-browser-data'
import type { KnowledgeSourceLocator, LocalKnowledgeDocumentAsk, NavigateWorkspace, WorkspaceView } from './types'
import { isSettingsSectionAnchor, localKnowledgeBackRoute, localKnowledgeViewForWorkspace, settingsRouteRedirect, workspaceViewForLocalKnowledge } from './management-navigation'

const CompanionWorkspace = lazy(() => import('@/features/companions/workspace'))
const AgentListPage = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentListPage })))
const AgentDetailPage = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentDetailPage })))
const KnowledgeDetailPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeDetailPage })))
const KnowledgeGraphPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeGraphPage })))
const SettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.SettingsPage })))
const ModelProvidersPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ModelProvidersPage })))
const YuxiSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.YuxiSettingsPage })))
const AgentSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.AgentSettingsPage })))
const CompanionsSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.CompanionsSettingsPage })))
const SecuritySettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.SecuritySettingsPage })))
const ConversationSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ConversationSettingsPage })))
const DataAboutSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.DataAboutSettingsPage })))
const ExtensionsSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ExtensionsSettingsPage })))
const ServicePage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ServicePage })))
const ModelServicePage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ModelServicePage })))
const LoginPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.LoginPage })))
const PluginDetailPage = lazy(() => import('@/features/plugins/plugin-detail-page'))
const PluginCenterPage = lazy(() => import('@/features/plugins').then((module) => ({ default: module.PluginCenterPage })))
const LocalKnowledgeWorkspace = lazy(() => import('@/features/local-knowledge').then((module) => ({ default: module.LocalKnowledgeWorkspace })))

export interface WorkspaceShellProps {
  activeView: WorkspaceView
  sidebarCollapsed: boolean
  onSidebar: () => void
  navigate: NavigateWorkspace
  activeEntityId?: string | null
  activeDocumentId?: string | null
  activeSourceLocator?: KnowledgeSourceLocator | null
  dark: boolean
  onDark: () => void
  onAskKnowledge: (knowledgeBase: KnowledgeBaseRecord) => void
  onAskLocalKnowledgeDocument: (payload: LocalKnowledgeDocumentAsk) => void
  localKnowledgeGateway?: LocalKnowledgeGateway
  pluginGateway?: PluginGateway
  companionSchedules?: WorkspaceSchedules
  localKnowledgeCreateRequest?: number
  onLocalKnowledgeCreateRequestHandled?: () => void
  onConfigurePluginAgent?: (plugin: PluginCardView) => void
}

function routeFallback() {
  return <div className="fox-route-loading"><span /><p>正在载入页面</p></div>
}

export function WorkspaceShell({
  activeView,
  sidebarCollapsed,
  onSidebar,
  navigate,
  activeEntityId,
  activeDocumentId,
  activeSourceLocator,
  dark,
  onDark,
  onAskKnowledge,
  onAskLocalKnowledgeDocument,
  localKnowledgeGateway,
  pluginGateway,
  companionSchedules,
  localKnowledgeCreateRequest,
  onLocalKnowledgeCreateRequestHandled,
  onConfigurePluginAgent,
}: WorkspaceShellProps) {
  const localKnowledgeView = localKnowledgeViewForWorkspace(activeView)
  const navigateLocalKnowledge = (view: LocalKnowledgeView, knowledgeBaseId?: string, documentId?: string) => {
    navigate(workspaceViewForLocalKnowledge(view), knowledgeBaseId, documentId ? { documentId } : undefined)
  }
  const backFromLocalKnowledge = () => {
    const target = localKnowledgeBackRoute(activeView, activeEntityId)
    navigate(target.view, target.entityId)
  }
  const pageProps = { sidebarCollapsed, onSidebar, navigate }
  // Legacy settings ids (restored history entries, external callers) render the
  // merged page that now hosts them, scrolled to their section.
  const settingsRedirect = settingsRouteRedirect(activeView)
  const settingsView = settingsRedirect?.view ?? activeView
  const settingsSection = settingsRedirect?.section ?? (isSettingsSectionAnchor(activeDocumentId) ? activeDocumentId : undefined)
  const initialPluginKind = isPluginSection(activeEntityId) ? activeEntityId : undefined

  let rawWorkspacePage: ReactNode = null
  if (isCompanionWorkspaceView(activeView) && activeView !== 'companions' && companionSchedules) {
    rawWorkspacePage = <CompanionWorkspace view={activeView} schedules={companionSchedules} navigate={navigate} />
  } else if (activeView === 'plugins' || (activeView === 'plugin-detail' && activeDocumentId === 'tool')) {
    rawWorkspacePage = <PluginCenterPage gateway={pluginGateway} initialKind={initialPluginKind} initialCategory={activeView === 'plugins' && activeEntityId !== 'tool' ? activeDocumentId ?? undefined : undefined} navigate={navigate} onConfigureAgent={onConfigurePluginAgent} />
  } else if ((activeView === 'plugin-detail' || activeView === 'settings-plugin-detail') && activeEntityId && isPluginSection(activeDocumentId)) {
    rawWorkspacePage = <PluginDetailPage key={activeView + activeEntityId} id={activeEntityId} kind={activeDocumentId} fromSettings={activeView === 'settings-plugin-detail'} navigate={navigate} gateway={pluginGateway} />
  } else if (localKnowledgeView) {
    rawWorkspacePage = (
      <LocalKnowledgeWorkspace
        view={localKnowledgeView}
        gateway={localKnowledgeGateway}
        knowledgeBaseId={activeEntityId ?? undefined}
        documentId={activeDocumentId ?? undefined}
        sourceLocator={activeSourceLocator ?? undefined}
        onNavigate={navigateLocalKnowledge}
        onNavigateWorkspace={navigate}
        onBack={backFromLocalKnowledge}
        onAskDocument={onAskLocalKnowledgeDocument}
        createRequest={localKnowledgeCreateRequest}
        onCreateRequestHandled={onLocalKnowledgeCreateRequestHandled}
      />
    )
  } else if (activeView === 'agents') {
    rawWorkspacePage = <AgentListPage {...pageProps} initialCategory={activeEntityId ?? undefined} />
  } else if (activeView === 'agent-detail') {
    rawWorkspacePage = <AgentDetailPage {...pageProps} agentId={activeEntityId} section={activeDocumentId ?? 'home'} />
  } else if (activeView === 'knowledge') {
    rawWorkspacePage = <LocalKnowledgeWorkspace view="list" gateway={localKnowledgeGateway} onNavigate={navigateLocalKnowledge} onNavigateWorkspace={navigate} createRequest={localKnowledgeCreateRequest} onCreateRequestHandled={onLocalKnowledgeCreateRequestHandled} />
  } else if (activeView === 'knowledge-detail') {
    rawWorkspacePage = <KnowledgeDetailPage {...pageProps} onAskKnowledge={onAskKnowledge} knowledgeId={activeEntityId} initialDocumentId={activeDocumentId} sourceLocator={activeSourceLocator} />
  } else if (activeView === 'knowledge-graph') {
    rawWorkspacePage = <KnowledgeGraphPage {...pageProps} knowledgeId={activeEntityId} />
  } else if (activeView === 'settings') {
    rawWorkspacePage = <SettingsPage {...pageProps} dark={dark} onDark={onDark} />
  } else if (settingsView === 'settings-ai') {
    rawWorkspacePage = <AgentSettingsPage {...pageProps} section={settingsSection} agentId={activeView === 'settings-ai' ? activeEntityId ?? undefined : undefined} />
  } else if (activeView === 'settings-companions') {
    rawWorkspacePage = <CompanionsSettingsPage {...pageProps} />
  } else if (settingsView === 'settings-security') {
    rawWorkspacePage = <SecuritySettingsPage {...pageProps} section={settingsSection} />
  } else if (activeView === 'settings-models') {
    rawWorkspacePage = <ModelProvidersPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} />
  } else if (activeView === 'settings-yuxi') {
    rawWorkspacePage = <YuxiSettingsPage {...pageProps} />
  } else if (settingsView === 'settings-conversation') {
    rawWorkspacePage = <ConversationSettingsPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} section={settingsSection} />
  } else if (settingsView === 'settings-data') {
    rawWorkspacePage = <DataAboutSettingsPage {...pageProps} section={settingsSection} />
  } else if (settingsView === 'settings-extensions' || (activeView === 'settings-plugin-detail' && activeDocumentId === 'tool')) {
    rawWorkspacePage = <ExtensionsSettingsPage {...pageProps} section={settingsSection} initialKind={initialPluginKind} gateway={pluginGateway} />
  } else if (activeView === 'service') {
    rawWorkspacePage = <ServicePage {...pageProps} />
  } else if (activeView === 'model-service') {
    rawWorkspacePage = <ModelServicePage {...pageProps} />
  } else if (activeView === 'login') {
    rawWorkspacePage = <LoginPage {...pageProps} />
  }

  if (!rawWorkspacePage) return null
  return <Suspense fallback={routeFallback()}>{rawWorkspacePage}</Suspense>
}
