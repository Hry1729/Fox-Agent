export type ApiResponse<T> =
  | { ok: true; data: T }
  | { ok: false; error: { code: string; message: string; retryable: boolean } }

export interface RuntimeInitialization {
  databaseReady: boolean
  runtimeAvailable: boolean
  defaultAgentId: string
}

export interface AgentRecord {
  id: string
  name: string
  description: string
  runtimeType: string
  defaultModel: string
  icon: string | null
  capabilities: unknown
  resources: {
    tools: AgentResourceRecord[]
    knowledges: AgentResourceRecord[]
    mcps: AgentResourceRecord[]
    skills: AgentResourceRecord[]
  }
  configurableItems: unknown
  isDefault: boolean
  available: boolean
}

export interface AgentResourceRecord {
  id: string
  name: string
  description: string
}

export interface SkillRecord {
  id: string
  name: string
  description: string
  version: string
  requiredTools: string[]
  sourcePath: string
  enabled: boolean
  valid: boolean
  validationError: string | null
  instructions: string
}

export interface McpServerRecord {
  id: string
  name: string
  command: string
  args: string[]
  enabled: boolean
  status: 'unknown' | 'connected' | 'unavailable' | 'disabled' | string
  credentialConfigured: boolean
  lastError: string | null
  lastCheckedAt: number | null
  createdAt: number
  updatedAt: number
}

export interface McpToolRecord {
  name: string
  description: string
  inputSchema: Record<string, unknown>
}

export interface McpConnectionTest {
  toolCount: number
  tools: McpToolRecord[]
}

export interface MaintenanceResult {
  path: string | null
  message: string
  files: number
  bytes: number
  restartRequired: boolean
}

export interface KnowledgePreviewCacheStatistics {
  totalFiles: number
  activeFiles: number
  totalBytes: number
  activeBytes: number
  limitBytes: number
}

export interface ProjectRecord {
  id: string
  name: string
  rootPath: string
  permissionMode: 'read_only' | 'ask' | 'allow'
  status: 'active' | 'missing' | 'revoked'
  createdAt: number
  updatedAt: number
  lastOpenedAt: number | null
}

export interface UsageAgentStat {
  agentId: string
  agentName: string
  conversationCount: number
  runCount: number
  totalTokens: number
}

export interface UsageDayStat {
  date: string
  conversationCount: number
  runCount: number
  totalTokens: number
}

export interface UsageStatistics {
  conversationCount: number
  completedRunCount: number
  userMessageCount: number
  activeDayCount: number
  inputTokens: number
  outputTokens: number
  totalTokens: number
  agents: UsageAgentStat[]
  days: UsageDayStat[]
}

export interface ProjectFileEntry {
  path: string
  name: string
  parent: string
  isDirectory: boolean
  byteSize: number
  modifiedAt: number | null
}

export interface ProjectFilePreview {
  path: string
  name: string
  content: string
  byteSize: number
  truncated: boolean
}

export interface ConversationSummary {
  id: string
  agentId: string
  agentName: string
  title: string
  projectId: string | null
  projectRoot: string | null
  status: string
  createdAt: number
  updatedAt: number
  lastMessageAt: number | null
}

export interface ConversationMessage {
  id: string
  conversationId: string
  runId: string | null
  role: 'user' | 'assistant' | 'tool' | 'system'
  kind: string
  content: string
  status: string
  ordinal: number
  createdAt: number
  updatedAt: number
}

export interface RunRecord {
  id: string
  conversationId: string
  runtimeSessionId: string | null
  status: string
  model: string
  startedAt: number | null
  finishedAt: number | null
  errorCode: string | null
  errorMessage: string | null
  lastSeq: number
}

export interface ConversationDetail {
  conversation: ConversationSummary
  messages: ConversationMessage[]
  runtimeEvents: RunEventRecord[]
  toolCalls: ToolCallRecord[]
  approvals: ApprovalRecord[]
  attachments: AttachmentRecord[]
  artifacts: ArtifactRecord[]
  knowledgeBindings: KnowledgeBindingRecord[]
  lastRun: RunRecord | null
  hasEarlierMessages: boolean
  goals: GoalRecord[]
  tasks: WorkTaskRecord[]
  evidence: TaskEvidenceRecord[]
}

export interface ConversationHistoryPage {
  messages: ConversationMessage[]
  runtimeEvents: RunEventRecord[]
  toolCalls: ToolCallRecord[]
  approvals: ApprovalRecord[]
  attachments: AttachmentRecord[]
  artifacts: ArtifactRecord[]
  hasEarlierMessages: boolean
}

export interface AttachmentRecord {
  id: string
  conversationId: string
  messageId: string | null
  displayName: string
  storagePath: string
  mediaType: string | null
  byteSize: number
  sha256: string | null
  status: string
  createdAt: number
}

export interface ArtifactRecord {
  id: string
  conversationId: string
  runId: string | null
  displayName: string
  artifactType: string
  storagePath: string
  mediaType: string | null
  byteSize: number
  sha256: string | null
  status: string
  createdAt: number
  updatedAt: number
}

export interface KnowledgeBindingRecord {
  conversationId: string
  serviceConnectionId: string
  knowledgeBaseId: string
  knowledgeBaseName: string | null
  enabled: boolean
  createdAt: number
  updatedAt: number
}

export interface ToolCallRecord {
  id: string
  runtimeToolCallId: string
  runId: string
  conversationId: string
  toolName: string
  input: unknown
  status: string
  result: unknown | null
  errorMessage: string | null
  executionLocation: 'runtime' | 'host' | string
  requiresApproval: boolean
  startedAt: number
  completedAt: number | null
  updatedAt: number
}

export interface ApprovalRecord {
  id: string
  toolCallId: string
  runId: string
  conversationId: string
  toolName: string
  status: 'pending' | 'approved' | 'denied' | 'cancelled' | 'expired'
  requestedAction: string
  request: {
    tool?: string
    title?: string
    target?: string
    summary?: string
    diff?: string | null
    command?: string | null
    cwd?: string | null
    [key: string]: unknown
  }
  decision: unknown | null
  requestedAt: number
  resolvedAt: number | null
}

export interface RunEventRecord {
  runId: string
  seq: number
  eventType: string
  event: RuntimeEventNotification['event']
  createdAt: number
}

export interface StartRunResult {
  run: RunRecord
  userMessage: ConversationMessage
  attachments: AttachmentRecord[]
}

export interface RuntimeStatus {
  state: string
  runtime: string
  runtimeVersion: string | null
  capabilities: Record<string, unknown>
  available: boolean
  currentConversationId: string | null
  activeRunId: string | null
  lastError: string | null
}

export interface RuntimeDiagnostics extends RuntimeStatus {
  protocol: string
  protocolVersion: number
  capabilityManifestVersion: number
  stderrTail: string[]
  sessionFileCount: number
  recoveryAttempts: number
  lastRecoveryAt: number | null
}

export interface RuntimeEventNotification {
  conversationId: string
  runtimeSessionId: string | null
  runId: string
  seq: number
  timestamp: string
  event: {
    type: string
    delta?: string
    code?: string
    message?: string
    [key: string]: unknown
  }
}

export interface YuxiServiceRecord {
  name: string
  baseUrl: string
  enabled: boolean
  connectionType: 'local' | 'lan' | 'remote'
  credentialConfigured: boolean
  lastStatus: string
  lastVersion: string | null
  lastLatencyMs: number | null
  lastCheckedAt: number | null
  createdAt: number
  updatedAt: number
}

export interface YuxiConnectionTest {
  ok: boolean
  baseUrl: string
  connectionType: 'local' | 'lan' | 'remote'
  status: string
  authenticated: boolean
  authStatus: 'authenticated' | 'not_configured'
  version: string | null
  message: string
  latencyMs: number
}

export interface YuxiUserRecord {
  uid: string
  username: string
  avatar: string | null
  role: string
  departmentId: number | null
  departmentName: string | null
}

export interface YuxiAgentRecord extends AgentRecord {
  slug: string
  backendId: string
}

export interface YuxiModelRecord {
  spec: string
  modelId: string
  displayName: string
  providerId: string
  providerDisplayName: string
}

export interface KnowledgeBaseRecord {
  id: string
  name: string
  description: string
  kbType: string | null
  status: string | null
  fileCount: number
  processedCount: number
  rowCount: number
  createdAt: string | null
  updatedAt: string | null
}

export interface KnowledgeDocumentRecord {
  id: string
  name: string
  parentId: string | null
  isFolder: boolean
  status: string | null
  size: number
  createdAt: string | null
  updatedAt: string | null
}

export interface KnowledgeDetailRecord {
  database: KnowledgeBaseRecord
  documents: KnowledgeDocumentRecord[]
}

export interface KnowledgeDocumentDownloadResult {
  path: string
  filename: string
  mediaType: string
  bytesWritten: number
  canOpenDirectly: boolean
}

export interface KnowledgePreviewCacheLease {
  cacheKey: string
  mediaType: string
  size: number
  cacheHit: boolean
}

export interface KnowledgeDocumentSourceMetadata {
  filename: string
  mediaType: string
  size: number
  sourceRevision: string
  versionId: string | null
  etag: string | null
  lastModified: string | null
  acceptsRanges: boolean
  previewApiVersion: number | null
  supportedPreviewVariants: string[]
  availableVariants: string[]
}

export interface KnowledgeDocumentReadingState {
  knowledgeBaseId: string
  documentId: string
  sourceRevision: string | null
  page: number
  scrollOffset: number
  zoom: number | null
  updatedAt: number
}

export interface KnowledgeDocumentBookmark {
  id: string
  knowledgeBaseId: string
  documentId: string
  sourceRevision: string | null
  page: number
  anchor: string
  excerpt: string
  label: string
  createdAt: number
  updatedAt: number
}

export interface KnowledgeDocumentAnnotation {
  id: string
  knowledgeBaseId: string
  documentId: string
  sourceRevision: string | null
  annotationType: 'note' | 'highlight'
  page: number
  anchor: string
  excerpt: string
  note: string
  color: 'blue' | 'yellow' | 'green' | 'red'
  createdAt: number
  updatedAt: number
}

export interface KnowledgeDocumentActivity {
  readingState: KnowledgeDocumentReadingState | null
  bookmarks: KnowledgeDocumentBookmark[]
  annotations: KnowledgeDocumentAnnotation[]
}

export interface KnowledgeDownloadProgress {
  downloadId: string
  documentId: string
  bytesWritten: number
  totalBytes: number | null
  status: 'downloading' | 'completed' | 'cancelled' | 'failed'
}

export interface ModelServiceRecord {
  name: string
  baseUrl: string
  modelId: string
  apiType: 'openai-completions' | 'anthropic-messages'
  contextWindow: number
  maxOutputTokens: number
  supportsImageInput: boolean
  enabled: boolean
  credentialConfigured: boolean
  connectionType: 'local' | 'lan' | 'remote'
  lastStatus: string
  lastLatencyMs: number | null
  lastCheckedAt: number | null
  createdAt: number
  updatedAt: number
}

export interface ModelConnectionTest {
  ok: boolean
  baseUrl: string
  connectionType: 'local' | 'lan' | 'remote'
  status: string
  latencyMs: number
  models: string[]
}

export interface ProviderModelRecord {
  id: string
  modelId: string
  displayName: string
  contextWindow: number
  maxOutputTokens: number
  supportsImageInput: boolean
  isDefault: boolean
}

export interface ModelProviderRecord {
  id: string
  name: string
  baseUrl: string
  apiType: 'openai-completions' | 'anthropic-messages'
  enabled: boolean
  isDefault: boolean
  credentialConfigured: boolean
  connectionType: 'local' | 'lan' | 'remote'
  lastStatus: string
  lastLatencyMs: number | null
  lastCheckedAt: number | null
  models: ProviderModelRecord[]
  createdAt: number
  updatedAt: number
}

// A0 Work Loop Types
export interface GoalRecord {
  id: string
  conversationId: string
  title: string
  objective: string
  acceptanceSummary: string | null
  status: 'proposed' | 'active' | 'blocked' | 'completed' | 'cancelled'
  version: number
  createdBy: string
  createdAt: string
  updatedAt: string
  completedAt: string | null
  blockedReason: string | null
}

export interface WorkTaskRecord {
  id: string
  goalId: string
  parentTaskId: string | null
  ordinal: number
  title: string
  detail: string | null
  status: 'queued' | 'in_progress' | 'completed' | 'blocked' | 'interrupted' | 'skipped'
  ownerRunId: string | null
  attempt: number
  version: number
  blockedReason: string | null
  createdAt: string
  updatedAt: string
  startedAt: string | null
  finishedAt: string | null
}

export interface TaskEvidenceRecord {
  id: string
  taskId: string
  sourceRunId: string | null
  evidenceType: 'tool_call' | 'trace_span' | 'test_result' | 'file_diff' | 'artifact' | 'user_confirmation' | 'external_reference'
  refKind: 'tool_call' | 'run_event' | 'artifact' | 'message' | 'source'
  refId: string
  summary: string
  metadata: Record<string, unknown>
  validityStatus: 'unverified' | 'valid' | 'stale' | 'missing' | 'invalid'
  traceId: string | null
  spanId: string | null
  checkedAt: string | null
  invalidReason: string | null
  createdAt: string
}
