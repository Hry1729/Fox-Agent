import { lazy, Suspense, type ReactNode } from 'react'
import type { KnowledgeBaseRecord } from '@/features/conversations/model/types'
import type { LocalKnowledgeGateway, LocalKnowledgeView } from '@/features/local-knowledge'
import type { PluginCardView, PluginGateway } from '@/features/plugins'
import type { KnowledgeSourceLocator, NavigateWorkspace, WorkspaceView } from './types'
import { localKnowledgeBackRoute, localKnowledgeViewForWorkspace, workspaceViewForLocalKnowledge } from './management-navigation'

const AgentListPage = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentListPage })))
const AgentDetailPage = lazy(() => import('@/features/agents/agent-pages').then((module) => ({ default: module.AgentDetailPage })))
const KnowledgeListPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeListPage })))
const KnowledgeDetailPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeDetailPage })))
const KnowledgeGraphPage = lazy(() => import('@/features/knowledge/knowledge-pages').then((module) => ({ default: module.KnowledgeGraphPage })))
const SettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.SettingsPage })))
const AiSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.AiSettingsPage })))
const ModelProvidersPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ModelProvidersPage })))
const YuxiSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.YuxiSettingsPage })))
const ProjectPermissionsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ProjectPermissionsPage })))
const ConversationSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ConversationSettingsPage })))
const UsageStatisticsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.UsageStatisticsPage })))
const ExtensionsSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.ExtensionsSettingsPage })))
const AboutSettingsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.AboutSettingsPage })))
const SkillsPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.SkillsPage })))
const McpPage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.McpPage })))
const MaintenancePage = lazy(() => import('@/features/settings/settings-pages').then((module) => ({ default: module.MaintenancePage })))
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
        onNavigate={navigateLocalKnowledge}
        onBack={backFromLocalKnowledge}
        createRequest={localKnowledgeCreateRequest}
        onCreateRequestHandled={onLocalKnowledgeCreateRequestHandled}
      />
    )
  } else if (activeView === 'agents') {
    rawWorkspacePage = <AgentListPage {...pageProps} />
  } else if (activeView === 'agent-detail') {
    rawWorkspacePage = <AgentDetailPage {...pageProps} agentId={activeEntityId} section={activeDocumentId ?? 'home'} />
  } else if (activeView === 'knowledge') {
    rawWorkspacePage = <KnowledgeListPage {...pageProps} />
  } else if (activeView === 'knowledge-detail') {
    rawWorkspacePage = <KnowledgeDetailPage {...pageProps} onAskKnowledge={onAskKnowledge} knowledgeId={activeEntityId} initialDocumentId={activeDocumentId} sourceLocator={activeSourceLocator} />
  } else if (activeView === 'knowledge-graph') {
    rawWorkspacePage = <KnowledgeGraphPage {...pageProps} knowledgeId={activeEntityId} />
  } else if (activeView === 'settings') {
    rawWorkspacePage = <SettingsPage {...pageProps} dark={dark} onDark={onDark} />
  } else if (activeView === 'settings-ai') {
    rawWorkspacePage = <AiSettingsPage {...pageProps} />
  } else if (activeView === 'settings-models') {
    rawWorkspacePage = <ModelProvidersPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} />
  } else if (activeView === 'settings-yuxi') {
    rawWorkspacePage = <YuxiSettingsPage {...pageProps} />
  } else if (activeView === 'settings-projects') {
    rawWorkspacePage = <ProjectPermissionsPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} />
  } else if (activeView === 'settings-conversation') {
    rawWorkspacePage = <ConversationSettingsPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} />
  } else if (activeView === 'settings-usage') {
    rawWorkspacePage = <UsageStatisticsPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} />
  } else if (activeView === 'settings-extensions') {
    rawWorkspacePage = <ExtensionsSettingsPage {...pageProps} />
  } else if (activeView === 'settings-about') {
    rawWorkspacePage = <AboutSettingsPage sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar} />
  } else if (activeView === 'skills') {
    rawWorkspacePage = <SkillsPage {...pageProps} />
  } else if (activeView === 'mcp') {
    rawWorkspacePage = <McpPage {...pageProps} />
  } else if (activeView === 'maintenance') {
    rawWorkspacePage = <MaintenancePage {...pageProps} />
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
