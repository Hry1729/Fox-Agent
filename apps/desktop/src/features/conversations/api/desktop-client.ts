import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { snapshotForRun } from '../model/kernel-snapshot'
import type { ReconciliationRequest, ReconciliationView, ReconciliationOptions } from '../model/reconciliation'
import type { KernelStateInvalidation, KernelModelPreview } from '../model/types'
import type {
  AgentRecord,
  ApiResponse,
  ConversationDetail,
  ConversationExpertBinding,
  ConversationSummary,
  ConversationHistoryPage,
  ChildRunNotification,
  ChildRunRecord,
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
  ProjectFileActionResponse,
  UsageStatistics,
  ObservabilityStatistics,
  EvaluationRunSummary,
  SkillRecord,
  McpServerRecord,
  McpConnectionTest,
  LifecycleHookRecord,
  SaveLifecycleHookInput,
  MaintenanceResult,
  WorkStateDiagnosticReport,
  WorkEventRecord,
  GoalRecord,
  PlanRevisionRecord,
  SetGoalRunningResult,
  ApprovalRecord,
  AttachmentRecord,
  ArtifactActionResponse,
  ArtifactInspectResponse,
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
  KnowledgeReference,
  YuxiAgentRecord,
  YuxiUserRecord,
  YuxiModelRecord,
  ExpertDisplaySnapshot,
  DesktopErrorDetails,
  MemoryEntityRecord,
  MemoryKind,
  MemoryRecallRecord,
  MemoryRevisionRecord,
  MemoryScope,
  ExpertPackagePreview,
  ExpertPackageVersionRecord,
  ExpertWorkflowSnapshot,
  ExpertTeamSnapshot,
  CreateDigitalColleagueInput,
  DigitalColleagueAuditRecord,
  DigitalColleagueChannelCreated,
  DigitalColleagueChannelRecord,
  DigitalColleagueRecord,
  DigitalColleagueScheduleRecord,
  DigitalColleagueTriggerRecord,
  AppNotificationRecord,
  GlobalSearchRecord,
  MessageFeedbackRecord,
  NotificationPreferencesRecord,
  ProjectManagementRecord,
} from '../model/types'
import {
  pluginActivationUpdateToWire,
  pluginCardFromWire,
  type PluginActivationUpdate,
  type PluginCardWire,
  type PluginCatalogDTO,
  type PluginCatalogQuery,
  type PluginCatalogWireDTO,
  type PluginInstallationsDTO,
  type PluginInstallationsWireDTO,
} from '../../plugins/model'

export const desktopRuntimeAvailable = isTauri()

export interface KnowledgeBindingsSetWireResponse {
  bindings: KnowledgeBindingRecord[]
  knowledgeReferences?: KnowledgeReference[]
}

export interface KnowledgeBindingsSetResult {
  bindings: KnowledgeBindingRecord[]
  knowledgeReferences: KnowledgeReference[]
}

type KnowledgeBindingsSetPayload = KnowledgeBindingsSetWireResponse | KnowledgeBindingRecord[]

export function knowledgeReferenceKey(reference: KnowledgeReference): string {
  return [
    reference.source,
    reference.providerKey ?? (reference.source === 'local' ? 'local' : reference.connectionId),
    reference.source === 'local' ? '' : reference.connectionId,
    reference.id,
  ].join(':')
}

export function normalizeKnowledgeReference(reference: KnowledgeReference): KnowledgeReference {
  if (reference.source === 'local') {
    return { ...reference, providerKey: reference.providerKey ?? 'local' }
  }
  return {
    ...reference,
    providerKey: reference.providerKey ?? reference.connectionId,
  }
}

export function knowledgeReferenceFromLegacyBinding(binding: KnowledgeBindingRecord): Extract<KnowledgeReference, { source: 'remote' }> {
  const connectionId = binding.serviceConnectionId || 'yuxi-primary'
  return normalizeKnowledgeReference({
    source: 'remote',
    connectionId,
    providerKey: connectionId,
    id: binding.knowledgeBaseId,
  }) as Extract<KnowledgeReference, { source: 'remote' }>
}

export function normalizeKnowledgeBindingsSetPayload(payload: KnowledgeBindingsSetPayload): KnowledgeBindingsSetResult {
  if (Array.isArray(payload)) {
    return {
      bindings: payload,
      knowledgeReferences: payload.map(knowledgeReferenceFromLegacyBinding),
    }
  }
  const bindings = Array.isArray(payload.bindings) ? payload.bindings : []
  return {
    bindings,
    knowledgeReferences: Array.isArray(payload.knowledgeReferences)
      ? payload.knowledgeReferences.map(normalizeKnowledgeReference)
      : bindings.map(knowledgeReferenceFromLegacyBinding),
  }
}

export interface LocalKnowledgeBaseDto {
  id: string
  name: string
  description: string | null
  activeIndexGeneration: string | null
  documentCount: number
  activeJobStatus: string | null
  /** Capability fields are optional for older Hosts; absence is treated as unavailable. */
  textIndexReady?: boolean
  vectorIndexReady?: boolean
  searchMode?: string | null
  configuredEmbeddingModelId?: string | null
  chunkSize?: number | null
  chunkOverlap?: number | null
  embeddingModel?: {
    id: string
    name: string
    version: string
    dimension: number
  } | null
  chunkCount?: number | null
  vectorCount?: number | null
  lastIndexedAt?: number | null
  fallbackReason?: string | null
  parserVersion?: string | null
  storageFreeBytes?: number | null
  storageTotalBytes?: number | null
  createdAt: number
  updatedAt: number
}

export interface LocalKnowledgeDocumentDto {
  id: string
  knowledgeBaseId: string
  displayName: string
  relativePath: string
  currentRevision: number
  fileSize: number
  mimeType: string
  parseStatus: string
  indexStatus: string
  chunkCount: number
  lastErrorCode: string | null
  lastErrorMessage: string | null
  createdAt: number
  updatedAt: number
}

export interface LocalKnowledgeFileSourceDto {
  id: string
  rootPath: string
  displayName: string
  fileCount: number
  totalSize: number
  lastScannedAt: number | null
  createdAt: number
  updatedAt: number
}

export interface LocalKnowledgeCatalogFileDto {
  id: string
  sourceId: string
  sourceName: string
  sourcePath: string
  absolutePath: string
  relativePath: string
  displayName: string
  extension: string
  mimeType: string
  fileSize: number
  modifiedAt: number | null
}

export interface LocalKnowledgeJobDto {
  id: string
  parentOperationId: string | null
  knowledgeBaseId: string
  jobType: string
  status: string
  stage: string | null
  progress: number
  retryCount: number
  lastSequence: number
  outcome: string | null
  generationId: string | null
  heartbeatAt: number | null
  checkpointJson: string | null
  errorCode: string | null
  errorMessage: string | null
  createdAt: number
  startedAt: number | null
  updatedAt: number
  completedAt: number | null
}

export interface LocalKnowledgeStorageStatusDto {
  rootPath: string
  databasePath: string
  writable: boolean
  schemaVersion: number
  pendingRootPath: string | null
  restartRequired: boolean
  migrationId?: string | null
}

export interface LocalKnowledgeStorageDirectoryDto {
  path: string | null
}

export interface LocalKnowledgeStorageMigrationDto {
  id: string
  sourcePath: string
  destinationPath: string
  status: string
  stage: string | null
  progress: number
  lastSequence: number
  errorCode: string | null
  errorMessage: string | null
  createdAt: number
  updatedAt: number
}

export interface PickedLocalKnowledgeFileDto {
  sourcePath: string
  name: string
  mimeType: string
  sizeBytes: number
  relativePath: string
}

export interface LocalKnowledgeFolderDto {
  knowledgeBaseId: string
  name: string
  relativePath: string
  documentCount: number
}

export interface LocalKnowledgeOperationAcceptedDto {
  operationId: string
  acceptedAt: number
}

export interface LocalEmbeddingModelDto {
  id: string
  name: string
  version: string
  languages: string[]
  dimension: number
  license: string
  sourceUrl: string
  status: string
  integrityStatus: string
  isDefault: boolean
  recommended: boolean
  sizeBytes: number
  installedAt: number | null
  packagePath: string | null
  loadReady: boolean
  lastErrorCode: string | null
  lastErrorMessage: string | null
  files: Array<{ name: string; downloadName: string; url: string; sha256: string; sizeBytes: number }>
  download: {
    id: string
    status: string
    progress: number
    downloadedBytes: number
    totalBytes: number
    currentFile: string | null
    errorCode: string | null
    errorMessage: string | null
    updatedAt: number
  } | null
}

export interface LocalEmbeddingModelTestDto {
  integrityVerified: boolean
  loadReady: boolean
  dimension: number
  elapsedMs: number
  message: string
  errorCode: string | null
  errorDetails: string | null
}

export interface LocalVectorBackendHealthDto {
  backend: string
  available: boolean
  readWriteVerified: boolean
  fallbackActive: boolean
  message: string
}

export class DesktopCommandError extends Error {
  readonly code: string
  readonly retryable: boolean

  constructor(details: DesktopErrorDetails) {
    super(details.message)
    this.name = 'DesktopCommandError'
    this.code = details.code
    this.retryable = details.retryable
  }
}

export function desktopErrorDetails(cause: unknown): DesktopErrorDetails {
  if (cause instanceof DesktopCommandError) {
    return { code: cause.code, message: cause.message, retryable: cause.retryable }
  }
  if (cause && typeof cause === 'object') {
    const value = cause as { code?: unknown; message?: unknown; retryable?: unknown }
    if (typeof value.code === 'string' && typeof value.message === 'string') {
      return { code: value.code, message: value.message, retryable: value.retryable === true }
    }
  }
  return {
    code: 'unknown',
    message: cause instanceof Error ? cause.message : String(cause),
    retryable: false,
  }
}

async function command<T>(name: string, request?: unknown): Promise<T> {
  const response = await invoke<ApiResponse<T>>(name, request === undefined ? {} : { request })
  if (!response.ok) throw new DesktopCommandError(response.error)
  return response.data
}

function normalizeExpertBinding(binding: ConversationExpertBinding): ConversationExpertBinding {
  const legacy = binding as ConversationExpertBinding & {
    displaySnapshotJson?: ExpertDisplaySnapshot
    packageSnapshotJson?: Record<string, unknown>
  }
  return {
    ...binding,
    displaySnapshot: binding.displaySnapshot ?? legacy.displaySnapshotJson,
    packageSnapshot: binding.packageSnapshot ?? legacy.packageSnapshotJson ?? {},
  }
}

/**
 * One durable mid-run supplementary request ("运行中补充要求"). The text never
 * grants tools and never rewrites a frozen request; the Host splices it into
 * model input only at the next dispatch boundary.
 * Status lifecycle: received → delivered → applied (cancelled terminal).
 */
export interface RunSteeringRecord {
  seq: number
  messageId: string
  content: string
  status: 'received' | 'delivered' | 'applied' | 'cancelled' | string
  receivedAt: number
  appliedAt: number | null
  appliedEventSeq: number | null
  appliedDispatchKey: string | null
}

export interface RunSteeringEnqueueResponse {
  runId: string
  seq: number
  messageId: string
  status: string
}

/**
 * The Run a steering listing belongs to, plus whether that Run can still accept
 * input. History is read-only: after a task ends (or the app reopens) the listing
 * still resolves the conversation's most recent authoritative Run so the four
 * states stay auditable, while `acceptsSteering` stays false and enqueueing
 * remains bound to an active Run.
 */
export interface RunSteeringListResponse {
  runId: string | null
  runState: string | null
  acceptsSteering: boolean
  messages: RunSteeringRecord[]
}

export interface ManagedFileVersion {
  id: string
  runId: string | null
  toolCallId: string | null
  tool: string
  storagePath: string
  displayName: string
  versionNo: number
  changeKind: 'created' | 'modified' | 'restored' | string
  beforeHash: string | null
  beforeSize: number | null
  afterHash: string | null
  afterSize: number | null
  backupPath: string | null
  restoredFromId: string | null
  createdAt: number
  /// Registered before-write backup still exists on disk.
  backupAvailable: boolean
  /// Hash of the file currently on disk, if present.
  currentHash: string | null
  /// Current bytes differ from the latest recorded version.
  drifted: boolean
  /// Size of the content this version records: the bytes a restore would write.
  restoreSize: number | null
  /// True only when this version's own verified content still resolves.
  canRestore: boolean
  /// Provenance: host_capture | office_connector | restore | legacy_unknown.
  sourceKind: string
  /// Why this version cannot be restored, when it cannot.
  restoreBlocker: string | null
}

export const desktopClient = {
  initialize: () => command<RuntimeInitialization>('runtime_initialize'),
  runtimeStatus: () => command<RuntimeStatus>('runtime_status'),
  runtimeDiagnostics: () => command<RuntimeDiagnostics>('runtime_diagnostics'),
  exportDiagnostics: () => command<MaintenanceResult>('diagnostics_export'),
  diagnoseWorkState: (conversationId: string) =>
    command<WorkStateDiagnosticReport>('diagnose_work_state', { conversationId }),
  exportWorkTrace: (conversationId: string) =>
    command<MaintenanceResult>('export_work_trace', { conversationId }),
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
  saveAgent: (request: import('../model/types').SaveAgentInput) => command<AgentRecord>('agent_save', request),
  copyAgent: (agentId: string, name?: string) => command<AgentRecord>('agent_copy', { agentId, name }),
  deleteAgent: (agentId: string) => command<boolean>('agent_delete', { agentId }),
  inspectExpertPackage: (expertPackage: Record<string, unknown>) =>
    command<ExpertPackagePreview>('expert_package_inspect', { package: expertPackage }),
  installExpertPackage: (expertPackage: Record<string, unknown>, expectedCurrentHash: string | null) =>
    command<AgentRecord>('expert_package_install', { package: expertPackage, expectedCurrentHash }),
  exportExpertPackage: (agentId: string) =>
    command<Record<string, unknown>>('expert_package_export', { agentId }),
  listExpertPackageVersions: (agentId: string) =>
    command<ExpertPackageVersionRecord[]>('expert_package_versions', { agentId }),
  rollbackExpertPackage: (expertId: string, version: string, expectedCurrentHash: string | null) =>
    command<AgentRecord>('expert_package_rollback', { expertId, version, expectedCurrentHash }),
  getExpertWorkflow: (conversationId: string) =>
    command<ExpertWorkflowSnapshot | null>('expert_workflow_get', { conversationId }),
  startExpertWorkflow: (conversationId: string, input: unknown = {}) =>
    command<ExpertWorkflowSnapshot>('expert_workflow_start', { conversationId, input }),
  resolveExpertWorkflowGate: (workflowRunId: string, stageId: string, decision: 'approved' | 'rejected', reason = '') =>
    command<ExpertWorkflowSnapshot>('expert_workflow_gate_resolve', { workflowRunId, stageId, decision, reason }),
  cancelExpertWorkflow: (workflowRunId: string, reason = '') =>
    command<ExpertWorkflowSnapshot>('expert_workflow_cancel', { workflowRunId, reason }),
  getExpertTeam: (conversationId: string) =>
    command<ExpertTeamSnapshot | null>('expert_team_get', { conversationId }),
  cancelExpertTeam: (teamRunId: string, reason = '') =>
    command<ExpertTeamSnapshot>('expert_team_cancel', { teamRunId, reason }),
  listDigitalColleagues: () =>
    command<DigitalColleagueRecord[]>('digital_colleagues_list'),
  createDigitalColleague: (request: CreateDigitalColleagueInput) =>
    command<DigitalColleagueRecord>('digital_colleague_create', request),
  updateDigitalColleague: (request: { colleagueId: string; name: string; objective: string; maxRunsPerDay: number; maxTokensPerDay: number; maxDurationMs: number; maxOutputTokens: number; maxToolCalls: number }) =>
    command<DigitalColleagueRecord>('digital_colleague_update', request),
  setDigitalColleaguePaused: (colleagueId: string, paused: boolean) =>
    command<DigitalColleagueRecord>('digital_colleague_set_paused', { colleagueId, paused }),
  revokeDigitalColleague: (colleagueId: string, reason = '') =>
    command<DigitalColleagueRecord>('digital_colleague_revoke', { colleagueId, reason }),
  listDigitalColleagueSchedules: (colleagueId: string) =>
    command<DigitalColleagueScheduleRecord[]>('digital_colleague_schedules_list', { colleagueId }),
  saveDigitalColleagueSchedule: (request: { scheduleId?: string; colleagueId: string; name: string; intervalSeconds: number; catchupWindowSeconds?: number; enabled?: boolean }) =>
    command<DigitalColleagueScheduleRecord>('digital_colleague_schedule_save', request),
  listDigitalColleagueChannels: (colleagueId: string) =>
    command<DigitalColleagueChannelRecord[]>('digital_colleague_channels_list', { colleagueId }),
  createDigitalColleagueChannel: (request: { colleagueId: string; name: string; channelKind: 'webhook' | 'im_bridge'; externalIdentity: string; rateLimitPerMinute?: number }) =>
    command<DigitalColleagueChannelCreated>('digital_colleague_channel_create', request),
  revokeDigitalColleagueChannel: (channelId: string) =>
    command<DigitalColleagueChannelRecord>('digital_colleague_channel_revoke', { channelId }),
  triggerDigitalColleague: (colleagueId: string, payload: unknown = {}, idempotencyKey?: string) =>
    command<DigitalColleagueTriggerRecord>('digital_colleague_trigger_manual', { colleagueId, payload, idempotencyKey }),
  triggerDigitalColleagueChannel: (request: { channelId: string; senderId: string; timestamp: number; idempotencyKey: string; signature: string; payload?: unknown }) =>
    command<DigitalColleagueTriggerRecord>('digital_colleague_trigger_channel', request),
  listDigitalColleagueTriggers: (colleagueId: string) =>
    command<DigitalColleagueTriggerRecord[]>('digital_colleague_triggers_list', { colleagueId }),
  getDigitalColleagueRunDetail: (runId: string) =>
    command<ConversationDetail>('digital_colleague_run_detail', { runId }),
  listDigitalColleagueAudit: (colleagueId: string) =>
    command<DigitalColleagueAuditRecord[]>('digital_colleague_audit_list', { colleagueId }),
  listSkills: (agentId: string) => command<SkillRecord[]>('skills_list', { agentId }),
  setSkillEnabled: (agentId: string, skillId: string, enabled: boolean) =>
    command<SkillRecord[]>('skill_set_enabled', { agentId, skillId, enabled }),
  listMcpServers: () => command<McpServerRecord[]>('mcp_servers_list'),
  saveMcpServer: (request: { id?: string; name: string; command: string; args: string[]; transport: McpServerRecord['transport']; endpointUrl?: string; definition?: string; environment?: Record<string, string>; clearEnvironment?: boolean }) =>
    command<McpServerRecord>('mcp_server_save', request),
  testMcpServer: (serverId: string) => command<McpConnectionTest>('mcp_server_test', { serverId }),
  setMcpServerEnabled: (serverId: string, enabled: boolean) =>
    command<McpServerRecord>('mcp_server_set_enabled', { serverId, enabled }),
  deleteMcpServer: (serverId: string) => command<boolean>('mcp_server_delete', { serverId }),
  listLifecycleHooks: () => command<LifecycleHookRecord[]>('lifecycle_hooks_list'),
  saveLifecycleHook: (request: SaveLifecycleHookInput) =>
    command<LifecycleHookRecord>('lifecycle_hook_save', request),
  setLifecycleHookEnabled: (hookId: string, enabled: boolean) =>
    command<LifecycleHookRecord>('lifecycle_hook_set_enabled', { hookId, enabled }),
  deleteLifecycleHook: (hookId: string) =>
    command<boolean>('lifecycle_hook_delete', { hookId }),
  pluginCatalogList: async (query: PluginCatalogQuery): Promise<PluginCatalogDTO> => {
    const dto = await command<PluginCatalogWireDTO>('plugin_catalog_list', query)
    return { ...dto, items: dto.items.map(pluginCardFromWire) }
  },
  pluginInstallationsList: async (): Promise<PluginInstallationsDTO> => {
    const dto = await command<PluginInstallationsWireDTO>('plugin_installations_list')
    return { ...dto, items: dto.items.map(pluginCardFromWire) }
  },
  pluginSetActivation: async (pluginId: string, update: PluginActivationUpdate) => {
    const card = await command<PluginCardWire>('plugin_set_activation', {
      pluginId,
      update: pluginActivationUpdateToWire(update),
    })
    return pluginCardFromWire(card)
  },
  listConversations: () => command<ConversationSummary[]>('conversations_list'),
  listArchivedConversations: () => command<ConversationSummary[]>('conversations_archived_list'),
  listTrashedConversations: () => command<ConversationSummary[]>('conversations_trashed_list'),
  usageStatistics: () => command<UsageStatistics>('usage_statistics'),
  observabilityStatistics: () => command<ObservabilityStatistics>('observability_statistics'),
  recordUiMetric: (runId: string, metric: 'ui.first_event' | 'ui.terminal_render', durationMs: number) =>
    command<void>('run_ui_metric_record', { runId, metric, durationMs }),
  runOfflineEvaluation: () => command<EvaluationRunSummary>('offline_evaluation_run'),
  getUserProfile: () => command<{ name: string; avatar: string } | null>('user_profile_get'),
  saveUserProfile: (profile: { name: string; avatar: string }) =>
    command<{ name: string; avatar: string }>('user_profile_save', profile),
  listMemories: (request: { agentId?: string; projectId?: string; includeDeleted?: boolean; query?: string } = {}) =>
    command<MemoryEntityRecord[]>('memories_list', request),
  createMemory: (request: { scope: MemoryScope; scopeKey?: string; kind: MemoryKind; canonicalKey: string; content: string; evidenceExcerpt?: string }) =>
    command<MemoryEntityRecord>('memory_create', request),
  confirmMemory: (memoryId: string, expectedVersion: number) =>
    command<MemoryEntityRecord>('memory_confirm', { memoryId, expectedVersion }),
  updateMemory: (request: { memoryId: string; kind: MemoryKind; canonicalKey: string; content: string; evidenceExcerpt?: string; expectedVersion: number }) =>
    command<MemoryEntityRecord>('memory_update', request),
  setMemoryEnabled: (memoryId: string, enabled: boolean, expectedVersion: number) =>
    command<MemoryEntityRecord>('memory_set_enabled', { memoryId, enabled, expectedVersion }),
  deleteMemory: (memoryId: string) =>
    command<MemoryEntityRecord>('memory_delete', { memoryId }),
  resolveMemoryConflict: (conflictId: string, decision: 'keep_existing' | 'accept_competing') =>
    command<MemoryEntityRecord>('memory_conflict_resolve', { conflictId, decision }),
  listMemoryRevisions: (memoryId: string) =>
    command<MemoryRevisionRecord[]>('memory_revisions_list', { memoryId }),
  listMemoryRecalls: (memoryId: string, limit = 50) =>
    command<MemoryRecallRecord[]>('memory_recalls_list', { memoryId, limit }),
  searchConversations: (query: string, limit = 50) =>
    command<ConversationSummary[]>('conversations_search', { query, limit }),
  listProjects: () => command<ProjectRecord[]>('projects_list'),
  listManagedProjects: () => command<ProjectManagementRecord[]>('projects_management_list'),
  updateProjectPath: (projectId: string, rootPath: string) =>
    command<ProjectManagementRecord>('project_path_update', { projectId, rootPath }),
  setProjectArchived: (projectId: string, archived: boolean) =>
    command<boolean>('project_archive', { projectId, archived }),
  openProjectRoot: (projectId: string) => command<boolean>('project_root_open', { projectId }),
  listAppNotifications: (unreadOnly = false, limit = 100) =>
    command<AppNotificationRecord[]>('app_notifications_list', { unreadOnly, limit }),
  publishAppNotification: (request: {
    kind: AppNotificationRecord['kind']
    severity: AppNotificationRecord['severity']
    title: string
    body: string
    mergeKey?: string
    sourceType?: string
    sourceId?: string
    workspaceView?: string
    entityId?: string
    status?: string
    action?: Record<string, unknown>
  }) => command<boolean>('app_notification_publish', request),
  setAppNotificationRead: (id: string, read: boolean) =>
    command<boolean>('app_notification_read', { id, read }),
  markAllAppNotificationsRead: () => command<number>('app_notifications_mark_all_read'),
  clearReadAppNotifications: () => command<number>('app_notifications_clear_read'),
  getNotificationPreferences: () => command<NotificationPreferencesRecord>('notification_preferences_get'),
  saveNotificationPreferences: (request: NotificationPreferencesRecord) =>
    command<NotificationPreferencesRecord>('notification_preferences_save', request),
  saveMessageFeedback: (messageId: string, sentiment: 'positive' | 'negative', category?: 'irrelevant' | 'code_error' | 'misunderstanding' | 'other', comment?: string) =>
    command<MessageFeedbackRecord>('message_feedback_save', { messageId, sentiment, category, comment }),
  globalSearch: (query: string, limit = 40) => command<GlobalSearchRecord[]>('app_global_search', { query, limit }),
  listProjectFiles: (conversationId: string) =>
    command<ProjectFileEntry[]>('project_files_list', { conversationId }),
  openProjectFolder: (conversationId: string) =>
    command<boolean>('project_folder_open', { conversationId }),
  readProjectFile: (conversationId: string, path: string) =>
    command<ProjectFilePreview>('project_file_read', { conversationId, path }),
  projectFileAction: (conversationId: string, path: string, action: ProjectFileActionResponse['action'], applicationId?: string) =>
    command<ProjectFileActionResponse>('project_file_action', { conversationId, path, action, applicationId }),
  inspectArtifact: (conversationId: string, artifactId: string) =>
    command<ArtifactInspectResponse>('artifact_inspect', { conversationId, artifactId }),
  artifactAction: (conversationId: string, artifactId: string, action: ArtifactActionResponse['action'], applicationId?: string) =>
    command<ArtifactActionResponse>('artifact_action', { conversationId, artifactId, action, applicationId }),
  pickProjectFolder: () => command<string | null>('project_folder_pick'),
  openExternalUrl: (url: string) => command<boolean>('external_url_open', { url }),
  updateProjectPermission: (projectId: string, permissionMode: ProjectRecord['permissionMode']) =>
    command<ProjectRecord>('project_permission_update', { projectId, permissionMode }),
  updateConversationPermission: (conversationId: string, permissionMode: ProjectRecord['permissionMode']) =>
    command<ConversationSummary>('conversation_permission_update', { conversationId, permissionMode }),
  deleteProject: (projectId: string) => command<boolean>('project_delete', { projectId }),
  createConversation: (request: { agentId: string; expertId?: string; title?: string; projectRoot?: string; permissionMode?: ProjectRecord['permissionMode'] }) =>
    command<ConversationSummary>('conversation_create', request),
  loadConversation: async (conversationId: string) => {
    const detail = await command<ConversationDetail>('conversation_load', { conversationId })
    if (detail.kernelSnapshot && !snapshotForRun(detail)) {
      throw new Error('运行状态快照无效，请刷新对话后重试')
    }
    const knowledgeReferences = Array.isArray(detail.knowledgeReferences)
      ? detail.knowledgeReferences.map(normalizeKnowledgeReference)
      : (detail.knowledgeBindings ?? []).map(knowledgeReferenceFromLegacyBinding)
    return {
      ...detail,
      knowledgeReferences,
      expertBindings: (detail.expertBindings ?? []).map(normalizeExpertBinding),
    }
  },
  bindConversationExpert: async (conversationId: string, expertId: string) =>
    normalizeExpertBinding(await command<ConversationExpertBinding>('conversation_expert_bind', { conversationId, expertId })),
  removeConversationExpert: async (conversationId: string) =>
    normalizeExpertBinding(await command<ConversationExpertBinding>('conversation_expert_remove', { conversationId })),
  loadConversationHistory: (conversationId: string, beforeOrdinal: number, limit = 100) =>
    command<ConversationHistoryPage>('conversation_history', { conversationId, beforeOrdinal, limit }),
  deleteConversation: (conversationId: string) =>
    command<boolean>('conversation_delete', { conversationId }),
  purgeConversation: (conversationId: string) =>
    command<boolean>('conversation_purge', { conversationId }),
  renameConversation: (conversationId: string, title: string) =>
    command<ConversationSummary>('conversation_rename', { conversationId, title }),
  setConversationPinned: (conversationId: string, pinned: boolean) =>
    command<ConversationSummary>('conversation_pin', { conversationId, pinned }),
  archiveConversation: (conversationId: string) =>
    command<ConversationSummary>('conversation_archive', { conversationId }),
  unarchiveConversation: (conversationId: string) =>
    command<ConversationSummary>('conversation_unarchive', { conversationId }),
  restoreConversation: (conversationId: string) =>
    command<ConversationSummary>('conversation_restore', { conversationId }),
  forkConversation: (conversationId: string, messageId: string, title?: string) =>
    command<ConversationSummary>('conversation_fork', { conversationId, messageId, title }),
  saveAttachments: (conversationId: string, files: Array<{ filename: string; mediaType?: string; dataUrl: string }>, messageId?: string) =>
    command<AttachmentRecord[]>('attachments_save', { conversationId, messageId, files }),
  setKnowledgeBindings: async (conversationId: string, knowledgeBases: Array<{ id: string; name: string }>) => {
    const payload = await command<KnowledgeBindingsSetPayload>('knowledge_bindings_set', { conversationId, knowledgeBases })
    return normalizeKnowledgeBindingsSetPayload(payload).bindings
  },
  setKnowledgeReferences: async (
    conversationId: string,
    knowledgeReferences: KnowledgeReference[],
    names: Record<string, string> = {},
  ): Promise<KnowledgeBindingsSetResult> => {
    const payload = await command<KnowledgeBindingsSetPayload>('knowledge_bindings_set', {
      conversationId,
      knowledgeReferences: knowledgeReferences.map((reference) => ({
        ...normalizeKnowledgeReference(reference),
        name: names[knowledgeReferenceKey(reference)] ?? reference.id,
      })),
    })
    return normalizeKnowledgeBindingsSetPayload(payload)
  },
  startRun: (request: { conversationId: string; text: string; runtimeText?: string; model?: string; attachmentIds?: string[]; budget?: import("../../chat/components/run-budget").RunBudgetSelection }) =>
    command<StartRunResult>('run_start', request),
  rewindRun: (request: { conversationId: string; messageId: string; text: string; runtimeText?: string; model?: string }) =>
    command<StartRunResult>('run_rewind', request),
  resumeRun: (request: { conversationId: string; parentRunId: string; text: string; answers: Record<string, string | string[]> }) =>
    command<StartRunResult>('run_resume', request),
  cancelRun: (runId: string) => command<boolean>('run_cancel', { runId }),
  // Durably accept a supplementary request for the conversation's active
  // authoritative Run. Re-submitting an existing messageId is idempotent.
  enqueueRunSteering: (conversationId: string, content: string, messageId?: string) =>
    command<RunSteeringEnqueueResponse>('run_steering_enqueue', { conversationId, content, messageId }),
  listRunSteering: (conversationId: string) =>
    command<RunSteeringListResponse>('run_steering_list', { conversationId }),
  // Append-only Host-managed file versions. Restore is a pure Host file copy
  // against a registered backup; it never replays tools or creates Runs.
  listManagedFileVersions: (conversationId: string, storagePath?: string) =>
    command<ManagedFileVersion[]>('managed_file_versions_list', {
      conversationId,
      storagePath: storagePath ?? null,
    }),
  restoreManagedFileVersion: (conversationId: string, versionId: string, force = false) =>
    command<ManagedFileVersion>('managed_file_restore', { conversationId, versionId, force }),
  reconciliationLoad: (request: ReconciliationRequest) => command<ReconciliationView>('kernel_reconciliation_load', request),
  reconciliationOptions: (request: ReconciliationRequest) => command<ReconciliationOptions>('kernel_reconciliation_options', request),
  reconciliationQuery: (request: ReconciliationRequest) => command<ReconciliationView>('kernel_reconciliation_query', request),
  reconciliationConfirm: (request: ReconciliationRequest) => command<ReconciliationView>('kernel_reconciliation_confirm', request),
  reconciliationResume: (request: ReconciliationRequest) => command<StartRunResult>('kernel_reconciliation_resume', request),
  cancelChildRun: (parentConversationId: string, childRunId: string) =>
    command<boolean>('child_run_cancel_by_user', { parentConversationId, childRunId }),
  resolveApproval: (approvalId: string, decision: import('../model/types').ApprovalDecision) =>
    command<boolean>('approval_resolve', { approvalId, decision }),
  resolveWorkModeConfirmation: (conversationId: string, goalId: string, expectedVersion: number, approved: boolean) =>
    command<GoalRecord>('work_mode_confirmation_resolve', { conversationId, goalId, expectedVersion, approved }),
  resolvePlanRevision: (conversationId: string, planRevisionId: string, decision: 'approved' | 'rejected') =>
    command<PlanRevisionRecord>('plan_revision_resolve', { conversationId, planRevisionId, decision }),
  deleteGoal: (conversationId: string, goalId: string) =>
    command<boolean>('goal_delete', { conversationId, goalId }),
  setGoalRunning: (conversationId: string, goalId: string, expectedVersion: number, running: boolean) =>
    command<SetGoalRunningResult>('goal_running_set', { conversationId, goalId, expectedVersion, running }),
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
  listLocalKnowledgeBases: () =>
    command<LocalKnowledgeBaseDto[]>('local_knowledge_bases_list'),
  listLocalKnowledgeFileSources: () =>
    command<LocalKnowledgeFileSourceDto[]>('local_knowledge_file_sources_list'),
  pickLocalKnowledgeSourceFolder: () =>
    command<string | null>('local_knowledge_source_folder_pick'),
  addLocalKnowledgeFileSource: (path: string) =>
    command<LocalKnowledgeFileSourceDto>('local_knowledge_file_source_add', { path }),
  rescanLocalKnowledgeFileSource: (id: string) =>
    command<LocalKnowledgeFileSourceDto>('local_knowledge_file_source_rescan', { id }),
  updateLocalKnowledgeFileSource: (id: string, displayName: string) =>
    command<LocalKnowledgeFileSourceDto>('local_knowledge_file_source_update', { id, displayName }),
  removeLocalKnowledgeFileSource: (id: string) =>
    command<boolean>('local_knowledge_file_source_remove', { id }),
  listLocalKnowledgeLocalFiles: (options: { sourceId?: string; query?: string; category?: string; limit?: number; offset?: number } = {}) =>
    command<LocalKnowledgeCatalogFileDto[]>('local_knowledge_local_files_list', options),
  readLocalKnowledgeLocalFileRange: (id: string, start: number, end: number) =>
    command<unknown>('local_knowledge_local_file_read', { id, start, end }),
  openLocalKnowledgeLocalFile: (id: string, reveal = false) =>
    command<boolean>('local_knowledge_local_file_open', { id, reveal }),
  getLocalKnowledgeBase: (id: string) =>
    command<LocalKnowledgeBaseDto>('local_knowledge_base_get', { id }),
  createLocalKnowledgeBase: (request: { name: string; description?: string | null; embeddingModelId?: string | null; chunkSize?: number; chunkOverlap?: number; searchMode?: string }) =>
    command<LocalKnowledgeBaseDto>('local_knowledge_base_create', request),
  updateLocalKnowledgeBase: (request: { id: string; name: string; description?: string | null; embeddingModelId?: string | null; chunkSize?: number; chunkOverlap?: number; searchMode?: string }) =>
    command<LocalKnowledgeBaseDto>('local_knowledge_base_update', request),
  deleteLocalKnowledgeBase: (id: string) =>
    command<boolean>('local_knowledge_base_delete', { id }),
  listLocalKnowledgeDocuments: (knowledgeBaseId: string, query?: string) =>
    command<LocalKnowledgeDocumentDto[]>('local_knowledge_documents_list', { knowledgeBaseId, query }),
  listLocalKnowledgeFolders: (knowledgeBaseId: string) =>
    command<LocalKnowledgeFolderDto[]>('local_knowledge_folders_list', { knowledgeBaseId }),
  createLocalKnowledgeFolder: (knowledgeBaseId: string, name: string, parentPath?: string) =>
    command<LocalKnowledgeFolderDto>('local_knowledge_folder_create', { knowledgeBaseId, name, parentPath }),
  readLocalKnowledgeDocumentFileRange: (knowledgeBaseId: string, documentId: string, start: number, end: number) =>
    command<unknown>('local_knowledge_document_file_read', { knowledgeBaseId, documentId, start, end }),
  openLocalKnowledgeDocumentFile: (knowledgeBaseId: string, documentId: string, reveal = false) =>
    command<boolean>('local_knowledge_document_file_open', { knowledgeBaseId, documentId, reveal }),
  pickLocalKnowledgeImportFiles: () =>
    command<PickedLocalKnowledgeFileDto[]>('local_knowledge_import_files_pick'),
  pickLocalKnowledgeImportFolder: () =>
    command<PickedLocalKnowledgeFileDto[]>('local_knowledge_import_folder_pick'),
  startLocalKnowledgeImport: (request: { knowledgeBaseId: string; files: Array<{ sourcePath: string; relativePath?: string }>; parserVersion?: string; chunkConfigHash?: string }) =>
    command<LocalKnowledgeOperationAcceptedDto>('local_knowledge_documents_import_start', request),
  listLocalKnowledgeJobs: (knowledgeBaseId?: string) =>
    command<LocalKnowledgeJobDto[]>('local_knowledge_jobs_list', knowledgeBaseId ? { knowledgeBaseId } : undefined),
  getLocalKnowledgeJob: (id: string) =>
    command<LocalKnowledgeJobDto>('local_knowledge_job_get', { id }),
  cancelLocalKnowledgeJob: (id: string) =>
    command<LocalKnowledgeJobDto>('local_knowledge_job_cancel', { id }),
  retryLocalKnowledgeJob: (id: string) =>
    command<LocalKnowledgeJobDto>('local_knowledge_job_retry', { id }),
  getLocalKnowledgeStorageStatus: () =>
    command<LocalKnowledgeStorageStatusDto>('local_knowledge_storage_status'),
  pickLocalKnowledgeStorageDirectory: () =>
    command<LocalKnowledgeStorageDirectoryDto | string | null>('local_knowledge_storage_directory_pick'),
  startLocalKnowledgeStorageMigration: (destinationPath: string) =>
    command<LocalKnowledgeOperationAcceptedDto>('local_knowledge_storage_migrate_start', { destinationPath }),
  getLocalKnowledgeStorageMigration: (id: string) =>
    command<LocalKnowledgeStorageMigrationDto>('local_knowledge_storage_migration_get', { id }),
  listLocalEmbeddingModels: () =>
    command<LocalEmbeddingModelDto[]>('local_embedding_models_list'),
  startLocalEmbeddingModelInstall: (modelId: string) =>
    command<LocalKnowledgeOperationAcceptedDto>('local_embedding_model_install_start', { modelId }),
  cancelLocalEmbeddingModelDownload: (id: string) =>
    command<boolean>('local_embedding_model_download_cancel', { id }),
  retryLocalEmbeddingModelDownload: (id: string) =>
    command<LocalKnowledgeOperationAcceptedDto>('local_embedding_model_download_retry', { id }),
  testLocalEmbeddingModel: (modelId: string) =>
    command<LocalEmbeddingModelTestDto>('local_embedding_model_test', { modelId }),
  setDefaultLocalEmbeddingModel: (modelId: string) =>
    command<boolean>('local_embedding_model_set_default', { modelId }),
  deleteLocalEmbeddingModel: (modelId: string) =>
    command<boolean>('local_embedding_model_delete', { modelId }),
  importLocalEmbeddingModelPackage: (packagePath: string) =>
    command<LocalEmbeddingModelDto>('local_embedding_model_import', { packagePath }),
  getLocalVectorBackendHealth: () =>
    command<LocalVectorBackendHealthDto>('local_vector_backend_health'),
  startLocalKnowledgeIndex: (knowledgeBaseId: string, rebuild = false) =>
    command<LocalKnowledgeOperationAcceptedDto>('local_knowledge_index_start', { knowledgeBaseId, rebuild }),
  testLocalKnowledgeRetrieval: (knowledgeBaseId: string, query: string, mode: string, limit?: number) =>
    command<unknown>('local_knowledge_retrieval_test', { knowledgeBaseId, query, mode, limit }),
  listLocalKnowledgeRetrievalCases: (knowledgeBaseId: string) =>
    command<unknown[]>('local_knowledge_retrieval_cases_list', { knowledgeBaseId }),
  saveLocalKnowledgeRetrievalCase: (request: { id?: string; knowledgeBaseId: string; question: string; expectedDocumentIds?: string[]; expectedKeywords?: string[] }) =>
    command<unknown>('local_knowledge_retrieval_case_save', request),
  deleteLocalKnowledgeRetrievalCase: (id: string) =>
    command<boolean>('local_knowledge_retrieval_case_delete', { id }),
  exportLocalKnowledgeRetrievalCases: (knowledgeBaseId: string) =>
    command<unknown>('local_knowledge_retrieval_cases_export', { knowledgeBaseId }),
  listLocalKnowledgeDocumentChunks: (knowledgeBaseId: string, documentId: string, limit?: number, offset?: number) =>
    command<unknown[]>('local_knowledge_document_chunks', { knowledgeBaseId, documentId, limit, offset }),
  deleteLocalKnowledgeDocument: (knowledgeBaseId: string, documentId: string) =>
    command<boolean>('local_knowledge_document_delete', { knowledgeBaseId, documentId }),
  reparseLocalKnowledgeDocument: (knowledgeBaseId: string, documentId: string) =>
    command<LocalKnowledgeOperationAcceptedDto>('local_knowledge_document_reparse', { knowledgeBaseId, documentId }),
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
  saveModelProvider: (request: { id?: string; name: string; icon?: string | null; baseUrl: string; apiType: 'openai-completions' | 'anthropic-messages'; isDefault: boolean; models: Array<{ id?: string; modelId: string; displayName: string; contextWindow: number; maxOutputTokens: number; supportsImageInput: boolean; isDefault: boolean }>; apiKey?: string; clearApiKey?: boolean }) =>
    command<ModelProviderRecord>('model_provider_save', request),
  deleteModelProvider: (providerId: string) => command<boolean>('model_provider_delete', { providerId }),
  listenRuntimeEvents: (handler: (event: RuntimeEventNotification) => void): Promise<UnlistenFn> =>
    listen<RuntimeEventNotification>('fox://runtime-event', ({ payload }) => handler(payload)),
  listenKernelStateInvalidations: (handler: (event: KernelStateInvalidation) => void): Promise<UnlistenFn> =>
    listen<KernelStateInvalidation>('fox://kernel-state-invalidated', ({ payload }) => handler(payload)),
  listenKernelModelPreviews: (handler: (event: KernelModelPreview) => void): Promise<UnlistenFn> =>
    listen<KernelModelPreview>('fox://kernel-model-preview', ({ payload }) => handler(payload)),
  listenChildRunUpdates: (handler: (event: ChildRunNotification) => void): Promise<UnlistenFn> =>
    listen<ChildRunNotification>('fox://child-run-updated', ({ payload }) => handler(payload)),
  listenWorkEvents: (handler: (event: WorkEventRecord) => void): Promise<UnlistenFn> =>
    listen<WorkEventRecord>('fox://work-event', ({ payload }) => handler(payload)),
  listenApprovalRequests: (handler: (approval: ApprovalRecord) => void): Promise<UnlistenFn> =>
    listen<ApprovalRecord>('fox://approval-requested', ({ payload }) => handler(payload)),
  listenApprovalResolved: (handler: (approval: ApprovalRecord) => void): Promise<UnlistenFn> =>
    listen<ApprovalRecord>('fox://approval-resolved', ({ payload }) => handler(payload)),
}
