import { createContext } from 'react'
import type { AgentRecord, ProjectRecord } from '@/features/conversations/model/types'

export const ComposerEntryContext = createContext<{
  projects: ProjectRecord[]
  onSelectProject?: (project: ProjectRecord) => void
  onAddProject?: () => void
  defaultExpert?: AgentRecord | null
  onViewDefaultExpert?: () => void
  onOpenModelSettings?: () => void
}>({ projects: [] })
