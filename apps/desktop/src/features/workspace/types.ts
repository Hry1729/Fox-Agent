export type CompanionWorkspaceView = 'companion-inbox' | 'companion-schedules' | 'companion-overview'

export function isCompanionWorkspaceView(view: WorkspaceView): view is 'companions' | CompanionWorkspaceView {
  return view === 'companions' || view === 'companion-inbox' || view === 'companion-schedules' || view === 'companion-overview'
}

export type WorkspaceView =
  | CompanionWorkspaceView
  | 'service'
  | 'model-service'
  | 'login'
  | 'chat'
  | 'companions'
  | 'notifications'
  | 'plugins'
  | 'plugin-detail'
  | 'settings-plugin-detail'
  | 'agents'
  | 'agent-detail'
  | 'knowledge'
  | 'knowledge-detail'
  | 'knowledge-graph'
  | 'local-knowledge-home'
  | 'local-files'
  | 'local-knowledge'
  | 'local-knowledge-detail'
  | 'local-knowledge-documents'
  | 'local-knowledge-import'
  | 'local-knowledge-jobs'
  | 'local-knowledge-models'
  | 'local-knowledge-retrieval'
  | 'local-knowledge-settings'
  | 'skills'
  | 'mcp'
  | 'maintenance'
  | 'settings'
  | 'onboarding'
  | 'settings-ai'
  | 'settings-models'
  | 'settings-search'
  | 'settings-yuxi'
  | 'settings-projects'
  | 'settings-conversation'
  | 'settings-companions'
  | 'settings-security'
  | 'settings-shortcuts'
  | 'settings-usage'
  | 'settings-extensions'
  | 'settings-about'
  | 'settings-data'

export interface WorkspaceNavigationContext {
  draftPrompt?: string
  conversationId?: string
  documentId?: string
  sourceLocator?: KnowledgeSourceLocator
}

export interface KnowledgeSourceLocator {
  chunkId?: string
  page?: number
  anchor?: string
  excerpt?: string
}

export interface LocalKnowledgeDocumentAsk {
  knowledgeBaseId: string
  knowledgeBaseName: string
  documentId: string
  documentName: string
  relativePath?: string
  mimeType?: string
  sizeBytes?: number
}

export type NavigateWorkspace = (view: WorkspaceView, entityId?: string, context?: WorkspaceNavigationContext) => void
