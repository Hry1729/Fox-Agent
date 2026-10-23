import { createContext } from 'react'
import type { AgentRecord, ProjectRecord } from '@/features/conversations/model/types'
import type { AgentBackendId, AgentBackendStatus } from '@/features/conversations/model/agent-backend'

export const ComposerEntryContext = createContext<{
  projects: ProjectRecord[]
  onSelectProject?: (project: ProjectRecord) => void
  onAddProject?: () => void
  defaultExpert?: AgentRecord | null
  onViewDefaultExpert?: () => void
  onOpenModelSettings?: () => void
  draftAgentBackend?: AgentBackendId
  agentBackendStatus?: AgentBackendStatus | null
  onSelectAgentBackend?: (backend: AgentBackendId) => void
}>({ projects: [] })
