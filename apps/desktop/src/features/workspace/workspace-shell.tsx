import { lazy, Suspense, type ReactNode } from 'react'
import type { KnowledgeBaseRecord } from '@/features/conversations/model/types'
import type { LocalKnowledgeGateway, LocalKnowledgeView } from '@/features/local-knowledge'
import type { PluginCardView, PluginGateway } from '@/features/plugins'
import type { KnowledgeSourceLocator, LocalKnowledgeDocumentAsk, NavigateWorkspace, WorkspaceView } from './types'
import { isSettingsSectionAnchor, localKnowledgeBackRoute, localKnowledgeViewForWorkspace, settingsRouteRedirect, workspaceViewForLocalKnowledge } from './management-navigation'

const AgentListPage = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentListPage })))
const AgentDetailPage = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentDetailPage })))
const KnowledgeDetailPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeDetailPage })))
const KnowledgeGraphPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeGraphPage })))
const SettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.SettingsPage })))
const ModelProvidersPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ModelProvidersPage })))
const YuxiSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.YuxiSettingsPage })))
const AgentSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.AgentSettingsPage })))
const ConversationSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ConversationSettingsPage })))
const DataAboutSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.DataAboutSettingsPage })))
const ExtensionsSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ExtensionsSettingsPage })))
const ServicePage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ServicePage })))
const ModelServicePage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ModelServicePage })))
const LoginPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.LoginPage })))
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
  const initialPluginKind = activeEntityId === 'tool' || activeEntityId === 'skill' || activeEntityId === 'mcp'
    ? activeEntityId
    : undefined

  let rawWorkspacePage: ReactNode = null
  if (activeView === 'plugins') {
    rawWorkspacePage = <PluginCenterPage gateway={pluginGateway} initialKind={initialPluginKind} onConfigureAgent={onConfigurePluginAgent} />
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
    rawWorkspacePage = <AgentListPage {...pageProps} />
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
  } else if (activeView === 'settings-models') {
    rawWorkspacePage = <ModelProvidersPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} />
  } else if (activeView === 'settings-yuxi') {
    rawWorkspacePage = <YuxiSettingsPage {...pageProps} />
  } else if (settingsView === 'settings-conversation') {
    rawWorkspacePage = <ConversationSettingsPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} section={settingsSection} />
  } else if (settingsView === 'settings-data') {
    rawWorkspacePage = <DataAboutSettingsPage {...pageProps} section={settingsSection} />
  } else if (settingsView === 'settings-extensions') {
    rawWorkspacePage = <ExtensionsSettingsPage {...pageProps} section={settingsSection} />
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
