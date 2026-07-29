export type WorkspaceView =
  | 'service'
  | 'model-service'
  | 'login'
  | 'chat'
  | 'agents'
  | 'agent-detail'
  | 'knowledge'
  | 'knowledge-detail'
  | 'knowledge-graph'
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
