export type WorkspaceView =
  | 'service'
  | 'model-service'
  | 'login'
  | 'chat'
  | 'plugins'
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
  | 'skills'
  | 'mcp'
  | 'maintenance'
  | 'settings'
  | 'onboarding'
  | 'settings-ai'
  | 'settings-models'
  | 'settings-yuxi'
  | 'settings-projects'
  | 'settings-conversation'
  | 'settings-usage'
  | 'settings-extensions'
  | 'settings-about'

export interface WorkspaceNavigationContext {
  documentId?: string
  sourceLocator?: KnowledgeSourceLocator
}

export interface KnowledgeSourceLocator {
  chunkId?: string
  page?: number
  anchor?: string
  excerpt?: string
}

export type NavigateWorkspace = (view: WorkspaceView, entityId?: string, context?: WorkspaceNavigationContext) => void
