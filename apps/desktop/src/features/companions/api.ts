import { command } from '@/features/conversations/api/desktop-client'
import type { AgentRecord, ConversationContextReset, MemoryEntityRecord } from '@/features/conversations/model/types'

export interface Companion {
  id: string
  conversationId: string
  templateAgentId: string
  displayName: string
  avatarRef: string
  duty: string
  personaOverride: string | null
  workspaceDir: string
  workspaceState: string
  workspaceError: string | null
  isDefault: boolean
  pinnedAt: number | null
  lastReadOrdinal: number
  modelProviderId: string | null
  modelRef: string | null
  unreadCount: number
  activeRunStatus: string | null
  archivedAt: number | null
  deletingAt: number | null
}

export interface CompanionsList {
  companions: Companion[]
  pendingDeletions: Array<{ companionId: string; displayName: string; error: string | null }>
}

export interface DeletePlan {
  companionId: string
  displayName: string
  workspacePath: string
  workspaceFiles: number
  workspaceBytes: number
  messages: number
  notes: number
  topics: number
  sharedMemories: number
  generations: number
  grantedProjects: string[]
  files: Array<{ kind: string; disposition: string; path: string; bytes: number }>
}

export interface TemplateStatus {
  latestHash: string | null
  boundSnapshot: Partial<AgentRecord> | null
  latestSnapshot: Partial<AgentRecord> | null
  updateAvailable: boolean
  dismissed: boolean
}

export interface NoteTopic {
  id: string; title: string; aliases: string[]; contentSha256: string; revision: number
}
export interface CompanionNotes {
  notes: MemoryEntityRecord[]
  usedChars: number
  limit: number
  draft: string | null
  topics: NoteTopic[]
  moves: Array<{ id: string; topicId: string; state: string }>
  requests: Array<{ id: string; memoryId: string; action: string; content: string; payload?: { content?: string; topic?: string } }>
  sharedRequests: Array<{ id: string; content: string; evidenceExcerpt: string; matches: Array<{ id: string; content: string }> }>
  requestHistory?: Array<CompanionNotes['requests'][number] & { status: 'open' | 'approved' | 'rejected' | 'stale'; resolvedAt: number | null }>
  sharedRequestHistory?: Array<CompanionNotes['sharedRequests'][number] & { status: 'open' | 'added' | 'replaced' | 'rejected' | 'stale'; resolvedAt: number | null }>
  writeReceipts?: Array<{ memoryId: string; runId: string; status: 'created' | 'undone' | 'changed' }>
}
export type CompanionGrant = { id: string; projectId: string; projectName: string; grantedAt: number }
export type RetainedWorkspace = { companionId: string; displayName: string; workspacePath: string }

export const direct = <T>(name: string, args?: Record<string, unknown>) => command<T>(name, args, true)
export const contacts = {
  list: () => command<CompanionsList>('companions_list'),
  open: (companionId: string) => command<Companion>('companion_open', { companionId }),
  add: (expertId: string) => command<Companion>('companion_add_from_expert', { expertId }),
  profile: (companionId: string, edit: Partial<Pick<Companion, 'displayName' | 'avatarRef' | 'duty' | 'personaOverride'>>) => command<Companion>('companion_update_profile', { companionId, ...edit }),
  setModel: (companionId: string, providerId: string | null, modelRef: string | null) => command<Companion>('companion_set_model', { companionId, providerId, modelRef }),
  pin: (companionId: string, pinned: boolean) => command<Companion>('companion_set_pinned', { companionId, pinned }),
  markRead: (companionId: string, ordinal: number) => command<Companion>('companion_mark_read', { companionId, ordinal }),
  archive: (companionId: string) => command<Companion>('companion_archive', { companionId }),
  restore: (companionId: string) => command<Companion>('companion_restore', { companionId }),
  template: (companionId: string) => command<TemplateStatus>('companion_template_status', { companionId }),
  decideTemplate: (companionId: string, expectedHash: string, adopt: boolean) => command(adopt ? 'companion_template_update' : 'companion_template_dismiss', { companionId, expectedHash }),
  repairContext: (conversationId: string) => command<ConversationContextReset>('companion_context_repair', { conversationId }),
  repairWorkspace: (companionId: string) => direct<string>('companion_workspace_repair', { companionId }),
  importFiles: (companionId: string, paths?: string[]) => direct<string[]>('companion_import_files', { companionId, paths }),
  exportFile: (companionId: string, path: string) => direct<string | null>('companion_export_file', { companionId, path }),
  preview: (companionId: string) => direct<{ planHash: string; plan: DeletePlan }>('companion_delete_preview', { companionId }),
  delete: (companionId: string, planHash: string) => direct('companion_delete', { companionId, planHash }),
  resume: (companionId: string) => direct('companion_delete_resume', { companionId }),
  retained: () => direct<RetainedWorkspace[]>('companion_retained_list'),
  notes: (companionId: string) => direct<CompanionNotes>('companion_notes_get', { companionId }),
  grants: (companionId: string) => direct<CompanionGrant[]>('companion_grants_list', { companionId }),
  setGrant: (companionId: string, projectId: string, grant: boolean) => direct(grant ? 'companion_grant_project' : 'companion_revoke_project', { companionId, projectId }),
}
