import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  AgentRecord,
  ApiResponse,
  ConversationDetail,
  ConversationSummary,
  ConversationHistoryPage,
  RuntimeEventNotification,
  RuntimeInitialization,
  RuntimeStatus,
  RuntimeDiagnostics,
  StartRunResult,
  YuxiConnectionTest,
  YuxiServiceRecord,
  ModelConnectionTest,
  ModelServiceRecord,
  ModelProviderRecord,
  ProjectRecord,
  ProjectFileEntry,
  ProjectFilePreview,
  UsageStatistics,
  SkillRecord,
  McpServerRecord,
  McpConnectionTest,
  MaintenanceResult,
  ApprovalRecord,
  AttachmentRecord,
  KnowledgeBaseRecord,
  KnowledgeDetailRecord,
  KnowledgeDocumentDownloadResult,
  KnowledgeDocumentSourceMetadata,
  KnowledgeDocumentActivity,
  KnowledgeDocumentReadingState,
  KnowledgeDocumentBookmark,
  KnowledgeDocumentAnnotation,
  KnowledgePreviewCacheLease,
  KnowledgePreviewCacheStatistics,
  KnowledgeDownloadProgress,
  KnowledgeBindingRecord,
  YuxiAgentRecord,
  YuxiUserRecord,
  YuxiModelRecord,
} from '../model/types'

export const desktopRuntimeAvailable = isTauri()

async function command<T>(name: string, request?: unknown): Promise<T> {
  const response = await invoke<ApiResponse<T>>(name, request === undefined ? {} : { request })
  if (!response.ok) throw new Error(response.error.message)
  return response.data
}

export const desktopClient = {
  initialize: () => command<RuntimeInitialization>('runtime_initialize'),
  runtimeStatus: () => command<RuntimeStatus>('runtime_status'),
  runtimeDiagnostics: () => command<RuntimeDiagnostics>('runtime_diagnostics'),
  exportDiagnostics: () => command<MaintenanceResult>('diagnostics_export'),
  createBackup: () => command<MaintenanceResult>('backup_create'),
  restoreBackup: (backupPath: string) => command<MaintenanceResult>('backup_restore', { backupPath }),
  cleanupData: (runtimeSessionDays = 30) => command<MaintenanceResult>('data_cleanup', { runtimeSessionDays }),
  knowledgePreviewCacheStatistics: () =>
    command<KnowledgePreviewCacheStatistics>('knowledge_preview_cache_statistics'),
  setKnowledgePreviewCacheLimit: (limitBytes: number) =>
    command<KnowledgePreviewCacheStatistics>('knowledge_preview_cache_limit_set', { limitBytes }),
  clearKnowledgePreviewCache: () =>
    command<MaintenanceResult>('knowledge_preview_cache_clear'),
  listAgents: () => command<AgentRecord[]>('agents_list'),
  listSkills: (agentId: string) => command<SkillRecord[]>('skills_list', { agentId }),
  setSkillEnabled: (agentId: string, skillId: string, enabled: boolean) =>
    command<SkillRecord[]>('skill_set_enabled', { agentId, skillId, enabled }),
  listMcpServers: () => command<McpServerRecord[]>('mcp_servers_list'),
  saveMcpServer: (request: { id?: string; name: string; command: string; args: string[]; environment?: Record<string, string>; clearEnvironment?: boolean }) =>
    command<McpServerRecord>('mcp_server_save', request),
  testMcpServer: (serverId: string) => command<McpConnectionTest>('mcp_server_test', { serverId }),
  setMcpServerEnabled: (serverId: string, enabled: boolean) =>
    command<McpServerRecord>('mcp_server_set_enabled', { serverId, enabled }),
  deleteMcpServer: (serverId: string) => command<boolean>('mcp_server_delete', { serverId }),
  listConversations: () => command<ConversationSummary[]>('conversations_list'),
  usageStatistics: () => command<UsageStatistics>('usage_statistics'),
  getUserProfile: () => command<{ name: string; avatar: string } | null>('user_profile_get'),
  saveUserProfile: (profile: { name: string; avatar: string }) =>
    command<{ name: string; avatar: string }>('user_profile_save', profile),
  searchConversations: (query: string, limit = 50) =>
    command<ConversationSummary[]>('conversations_search', { query, limit }),
  listProjects: () => command<ProjectRecord[]>('projects_list'),
  listProjectFiles: (conversationId: string) =>
    command<ProjectFileEntry[]>('project_files_list', { conversationId }),
  readProjectFile: (conversationId: string, path: string) =>
    command<ProjectFilePreview>('project_file_read', { conversationId, path }),
  pickProjectFolder: () => command<string | null>('project_folder_pick'),
  updateProjectPermission: (projectId: string, permissionMode: ProjectRecord['permissionMode']) =>
    command<ProjectRecord>('project_permission_update', { projectId, permissionMode }),
  createConversation: (request: { agentId: string; title?: string; projectRoot?: string; permissionMode?: ProjectRecord['permissionMode'] }) =>
    command<ConversationSummary>('conversation_create', request),
  loadConversation: (conversationId: string) =>
    command<ConversationDetail>('conversation_load', { conversationId }),
  loadConversationHistory: (conversationId: string, beforeOrdinal: number, limit = 100) =>
    command<ConversationHistoryPage>('conversation_history', { conversationId, beforeOrdinal, limit }),
  deleteConversation: (conversationId: string) =>
    command<boolean>('conversation_delete', { conversationId }),
  renameConversation: (conversationId: string, title: string) =>
    command<ConversationSummary>('conversation_rename', { conversationId, title }),
  saveAttachments: (conversationId: string, files: Array<{ filename: string; mediaType?: string; dataUrl: string }>, messageId?: string) =>
    command<AttachmentRecord[]>('attachments_save', { conversationId, messageId, files }),
  setKnowledgeBindings: (conversationId: string, knowledgeBases: Array<{ id: string; name: string }>) =>
    command<KnowledgeBindingRecord[]>('knowledge_bindings_set', { conversationId, knowledgeBases }),
  startRun: (request: { conversationId: string; text: string; runtimeText?: string; model?: string; attachmentIds?: string[] }) =>
    command<StartRunResult>('run_start', request),
  resumeRun: (request: { conversationId: string; parentRunId: string; text: string; answers: Record<string, string | string[]> }) =>
    command<StartRunResult>('run_resume', request),
  cancelRun: (runId: string) => command<boolean>('run_cancel', { runId }),
  resolveApproval: (approvalId: string, approved: boolean) =>
    command<boolean>('approval_resolve', { approvalId, approved }),
  getYuxiService: () => command<YuxiServiceRecord | null>('yuxi_service_get'),
  saveYuxiService: (request: { name: string; baseUrl: string; accessToken?: string; clearAccessToken?: boolean }) =>
    command<YuxiServiceRecord>('yuxi_service_save', request),
  testYuxiService: (baseUrl?: string, accessToken?: string) =>
    command<YuxiConnectionTest>('yuxi_service_test', { baseUrl, accessToken }),
  loginYuxi: (username: string, password: string) =>
    command<YuxiUserRecord>('yuxi_login', { username, password }),
  getYuxiUser: () => command<YuxiUserRecord>('yuxi_current_user'),
  syncYuxiAgents: () => command<YuxiAgentRecord[]>('yuxi_agents_sync'),
  listYuxiModels: () => command<YuxiModelRecord[]>('yuxi_models_list'),
  listKnowledgeBases: () => command<KnowledgeBaseRecord[]>('knowledge_bases_list'),
  getKnowledgeDetail: (knowledgeBaseId: string) =>
    command<KnowledgeDetailRecord>('knowledge_detail', { knowledgeBaseId }),
  getKnowledgeDocument: (knowledgeBaseId: string, documentId: string) =>
    command<unknown>('knowledge_document_content', { knowledgeBaseId, documentId }),
  getKnowledgeDocumentActivity: (knowledgeBaseId: string, documentId: string) =>
    command<KnowledgeDocumentActivity>('knowledge_document_activity_get', { knowledgeBaseId, documentId }),
  saveKnowledgeDocumentReadingState: (request: { knowledgeBaseId: string; documentId: string; sourceRevision?: string | null; page: number; scrollOffset: number; zoom?: number | null }) =>
    command<KnowledgeDocumentReadingState>('knowledge_document_reading_state_save', request),
  saveKnowledgeDocumentBookmark: (request: { id?: string; knowledgeBaseId: string; documentId: string; sourceRevision?: string | null; page: number; anchor?: string; excerpt?: string; label?: string }) =>
    command<KnowledgeDocumentBookmark>('knowledge_document_bookmark_save', request),
  deleteKnowledgeDocumentBookmark: (knowledgeBaseId: string, documentId: string, id: string) =>
    command<boolean>('knowledge_document_bookmark_delete', { knowledgeBaseId, documentId, id }),
  saveKnowledgeDocumentAnnotation: (request: { id?: string; knowledgeBaseId: string; documentId: string; sourceRevision?: string | null; annotationType?: 'note' | 'highlight'; page: number; anchor?: string; excerpt?: string; note: string; color?: 'blue' | 'yellow' | 'green' | 'red' }) =>
    command<KnowledgeDocumentAnnotation>('knowledge_document_annotation_save', request),
  deleteKnowledgeDocumentAnnotation: (knowledgeBaseId: string, documentId: string, id: string) =>
    command<boolean>('knowledge_document_annotation_delete', { knowledgeBaseId, documentId, id }),
  getKnowledgeDocumentSourceMetadata: (knowledgeBaseId: string, documentId: string) =>
    command<KnowledgeDocumentSourceMetadata>('knowledge_document_source_metadata', { knowledgeBaseId, documentId }),
  readKnowledgeDocumentRange: (knowledgeBaseId: string, documentId: string, start: number, end: number, sourceRevision: string) =>
    invoke<ArrayBuffer>('knowledge_document_range', { request: { knowledgeBaseId, documentId, start, end, sourceRevision } }),
  acquireKnowledgePreviewCache: (request: { operationId: string; knowledgeBaseId: string; documentId: string; sourceRevision: string; mediaType: string; size: number; filename?: string }) =>
    command<KnowledgePreviewCacheLease>('knowledge_preview_cache_acquire', request),
  registerKnowledgePreviewCacheOperation: (operationId: string) =>
    command<boolean>('knowledge_preview_cache_operation_register', { operationId }),
  finishKnowledgePreviewCacheOperation: (operationId: string) =>
    command<boolean>('knowledge_preview_cache_operation_finish', { operationId }),
  cancelKnowledgePreviewCache: (operationId: string) =>
    command<boolean>('knowledge_preview_cache_cancel', { operationId }),
  readKnowledgePreviewCache: (cacheKey: string, start: number, end: number) =>
    invoke<ArrayBuffer>('knowledge_preview_cache_read', { request: { cacheKey, start, end } }),
  releaseKnowledgePreviewCache: (cacheKey: string) =>
    command<boolean>('knowledge_preview_cache_release', { cacheKey }),
  openKnowledgePreviewCache: (cacheKey: string) =>
    command<boolean>('knowledge_preview_cache_open', { cacheKey }),
  downloadKnowledgeDocument: (downloadId: string, knowledgeBaseId: string, documentId: string, filename: string) =>
    command<KnowledgeDocumentDownloadResult | null>('knowledge_document_download', { downloadId, knowledgeBaseId, documentId, filename }),
  cancelKnowledgeDocumentDownload: (downloadId: string) =>
    command<boolean>('knowledge_document_download_cancel', { downloadId }),
  openDownloadedFile: (path: string, reveal = false) =>
    command<boolean>('downloaded_file_open', { path, reveal }),
  listenKnowledgeDownloadProgress: (handler: (progress: KnowledgeDownloadProgress) => void): Promise<UnlistenFn> =>
    listen<KnowledgeDownloadProgress>('fox://knowledge-download-progress', ({ payload }) => handler(payload)),
  queryKnowledge: (knowledgeBaseId: string, query: string) =>
    command<unknown>('knowledge_query', { knowledgeBaseId, query }),
  getKnowledgeGraph: (knowledgeBaseId: string, keyword = '', maxDepth = 2, maxNodes = 100) =>
    command<unknown>('knowledge_graph', { knowledgeBaseId, keyword, maxDepth, maxNodes }),
  getKnowledgeGraphLabels: (knowledgeBaseId: string) =>
    command<unknown>('knowledge_graph_labels', { knowledgeBaseId }),
  getKnowledgeGraphStats: (knowledgeBaseId: string) =>
    command<unknown>('knowledge_graph_stats', { knowledgeBaseId }),
  getModelService: () => command<ModelServiceRecord | null>('model_service_get'),
  saveModelService: (request: { name: string; baseUrl: string; modelId: string; apiType: 'openai-completions' | 'anthropic-messages'; contextWindow: number; maxOutputTokens: number; supportsImageInput: boolean; apiKey?: string; clearApiKey?: boolean }) =>
    command<ModelServiceRecord>('model_service_save', request),
  testModelService: (baseUrl?: string, apiKey?: string, apiType?: 'openai-completions' | 'anthropic-messages', modelId?: string) =>
    command<ModelConnectionTest>('model_service_test', { baseUrl, apiKey, apiType, modelId }),
  listModelProviders: () => command<ModelProviderRecord[]>('model_providers_list'),
  saveModelProvider: (request: { id?: string; name: string; baseUrl: string; apiType: 'openai-completions' | 'anthropic-messages'; isDefault: boolean; models: Array<{ id?: string; modelId: string; displayName: string; contextWindow: number; maxOutputTokens: number; supportsImageInput: boolean; isDefault: boolean }>; apiKey?: string; clearApiKey?: boolean }) =>
    command<ModelProviderRecord>('model_provider_save', request),
  deleteModelProvider: (providerId: string) => command<boolean>('model_provider_delete', { providerId }),
  listenRuntimeEvents: (handler: (event: RuntimeEventNotification) => void): Promise<UnlistenFn> =>
    listen<RuntimeEventNotification>('fox://runtime-event', ({ payload }) => handler(payload)),
  listenApprovalRequests: (handler: (approval: ApprovalRecord) => void): Promise<UnlistenFn> =>
    listen<ApprovalRecord>('fox://approval-requested', ({ payload }) => handler(payload)),
  listenApprovalResolved: (handler: (approval: ApprovalRecord) => void): Promise<UnlistenFn> =>
    listen<ApprovalRecord>('fox://approval-resolved', ({ payload }) => handler(payload)),
}
