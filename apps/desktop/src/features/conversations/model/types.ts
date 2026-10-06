import type { KernelRunSnapshot } from '../../../../../../packages/fox-engine-protocol'

export type { KernelRunSnapshot, KernelStateInvalidation, KernelModelPreview } from '../../../../../../packages/fox-engine-protocol'

export interface DesktopErrorDetails {
  code: string
  message: string
  retryable: boolean
}

export interface ExpertOperationResult {
  success: boolean
  error: DesktopErrorDetails | null
}

export type ApiResponse<T> =
  | { ok: true; data: T }
  | { ok: false; error: DesktopErrorDetails }

export interface RuntimeInitialization {
  databaseReady: boolean
  runtimeAvailable: boolean
  defaultAgentId: string
}

export type AgentKind = 'assistant' | 'expert' | 'worker'
export type AgentInvocationMode = 'primary' | 'inline' | 'child'
export type AgentVisibility = 'chat_selector' | 'expert_center' | 'hidden'

export interface AgentRecord {
  id: string
  name: string
  description: string
  runtimeType: string
  agentKind?: AgentKind
  invocationMode?: AgentInvocationMode
  visibility?: AgentVisibility
  defaultModel: string
  icon: string | null
  category: string
  openingSuggestions: string[]
  systemPrompt: string
  isBuiltin: boolean
  packageVersion: string
  packageSource: 'builtin' | 'local' | 'imported' | 'remote'
  packageId: string | null
  packageHash: string | null
  packageManifest: ExpertPackageManifest
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
  transport: 'stdio' | 'streamable_http' | 'openapi'
  endpointUrl: string | null
  definition: string | null
  enabled: boolean
  status: 'unknown' | 'connected' | 'unavailable' | 'disabled' | string
  credentialConfigured: boolean
  lastError: string | null
  lastCheckedAt: number | null
  lastLatencyMs: number | null
  toolCount: number | null
  consecutiveFailures: number
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
  latencyMs: number
  transport: McpServerRecord['transport']
}

export interface LifecycleHookRecord {
  id: string
  name: string
  event: 'before_run' | 'before_tool' | 'after_tool' | 'after_run'
  matcher: string
  action: 'block' | 'require_approval' | 'annotate'
  reason: string
  enabled: boolean
  priority: number
  createdAt: number
  updatedAt: number
}

export interface SaveLifecycleHookInput {
  id?: string
  name: string
  event: LifecycleHookRecord['event']
  matcher: string
  action: LifecycleHookRecord['action']
  reason: string
  enabled?: boolean
  priority?: number
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
  pinned?: boolean
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
  cacheReadTokens: number
  cacheWriteTokens: number
  totalTokens: number
  agents: UsageAgentStat[]
  days: UsageDayStat[]
}

export interface ProjectManagementRecord {
  id: string
  name: string
  rootPath: string
  permissionMode: ProjectRecord['permissionMode']
  status: ProjectRecord['status']
  pathExists: boolean
  isGitRepository: boolean
  gitBranch: string | null
  diskBytes: number | null
  conversationCount: number
  artifactCount: number
  recentConversationTitle: string | null
  recentArtifactName: string | null
  lastOpenedAt: number | null
  archivedAt: number | null
  updatedAt: number
}

export interface AppNotificationRecord {
  id: string
  mergeKey: string
  kind: 'approval' | 'question' | 'progress' | 'completed' | 'failed'
  severity: 'quiet' | 'normal' | 'high'
  title: string
  body: string
  sourceType: string
  sourceId: string
  workspaceView: string | null
  entityId: string | null
  progress: number | null
  status: string
  action: Record<string, unknown>
  readAt: number | null
  createdAt: number
  updatedAt: number
}

export interface NotificationPreferencesRecord {
  systemPopup: boolean
  sound: boolean
  soundId: 'soft' | 'chime' | 'pop' | 'signal'
  badge: boolean
  quietProgress: boolean
  updatedAt: number
}

export interface MessageFeedbackRecord {
  id: string
  messageId: string
  conversationId: string
  runId: string | null
  sentiment: 'positive' | 'negative'
  category: 'irrelevant' | 'code_error' | 'misunderstanding' | 'other' | null
  comment: string | null
  model: string | null
  createdAt: number
  updatedAt: number
}

export interface GlobalSearchRecord {
  id: string
  kind: 'conversation' | 'project' | 'agent' | 'digital_colleague' | 'knowledge_base' | 'knowledge_document' | 'task' | 'artifact'
  title: string
  subtitle: string
  workspaceView: string
  entityId: string | null
  documentId: string | null
  updatedAt: number
}

export interface TraceRunSummary {
  runId: string
  conversationId: string
  traceId: string
  rootSpanId: string
  status: string
  model: string
  startedAt: number
  finishedAt: number | null
  totalDurationMs: number | null
  planningDurationMs: number
  modelDurationMs: number
  toolDurationMs: number
  uiDurationMs: number
  spanCount: number
}

export interface LatencyMetric {
  operation: string
  sampleCount: number
  averageMs: number
  p50Ms: number
  p95Ms: number
  maxMs: number
}

export interface EvaluationSuiteSummary {
  name: string
  source: string | null
  sourceUrl: string | null
  total: number
  passed: number
  failed: number
}

export interface EvaluationRunSummary {
  id: string
  generatedAt: string
  recordedAt: number
  durationMs: number
  reportHash: string
  suites: number
  total: number
  passed: number
  failed: number
  suiteResults: EvaluationSuiteSummary[]
}

export interface ObservabilityStatistics {
  traceSchemaVersion: number
  tracedRunCount: number
  totalRunCount: number
  traceCoveragePercent: number
  recentRuns: TraceRunSummary[]
  latencyMetrics: LatencyMetric[]
  evaluationHistory: EvaluationRunSummary[]
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

export interface ProjectFileActionResponse {
  action: 'inspect' | 'open' | 'open_with' | 'reveal' | 'copy_path'
  completed: boolean
  applications: ArtifactApplication[]
}

export interface ConversationSummary {
  id: string
  agentId: string
  agentName: string
  title: string
  projectId: string | null
  projectRoot: string | null
  permissionMode: ProjectRecord['permissionMode']
  status: string
  pinned?: boolean
  archived?: boolean
  archivedAt?: number | null
  trashedAt?: number | null
  parentConversationId?: string | null
  forkedFromMessageId?: string | null
  lineageRootId?: string
  createdAt: number
  updatedAt: number
  lastMessageAt: number | null
  /** This conversation's own non-terminal Run, not the currently open one. */
  activeRunId?: string | null
  activeRunStatus?: string | null
  /** True while that Run still has a pending approval. */
  awaitingApproval?: boolean
  lastRunId?: string | null
  lastRunStatus?: string | null
  awaitingReply?: boolean
}

export interface ConversationMessage {
  /** Ephemeral display cursor; never sent back as persisted model history. */
  kernelPreview?: { checkpointSeq: number; revision: number; reasoning?: string }
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

export type MemoryScope = 'global' | 'agent' | 'project'
export type MemoryKind = 'preference' | 'identity' | 'project' | 'workflow' | 'fact' | 'other'
export type MemoryState = 'candidate' | 'confirmed' | 'conflict'

export interface MemoryEntityRecord {
  id: string
  scope: MemoryScope
  scopeKey: string
  kind: MemoryKind
  canonicalKey: string
  content: string
  state: MemoryState
  enabled: boolean
  sourceConversationId: string | null
  sourceMessageId: string | null
  sourceRunId: string | null
  evidenceExcerpt: string
  createdBy: 'user' | 'agent' | 'system'
  confidence: number
  version: number
  createdAt: number
  updatedAt: number
  confirmedAt: number | null
  disabledAt: number | null
  deletedAt: number | null
  openConflictId: string | null
  recallCount: number
  lastRecalledAt: number | null
}

export interface MemoryRevisionRecord {
  id: string
  memoryId: string
  actor: 'user' | 'agent' | 'system'
  action: string
  before: Record<string, unknown> | null
  after: Record<string, unknown> | null
  createdAt: number
}

export interface MemoryRecallRecord {
  id: string
  memoryId: string
  conversationId: string
  runId: string | null
  query: string
  reason: string
  score: number
  rank: number
  evidenceExcerpt: string
  recalledAt: number
}

export interface ExpertDisplaySnapshot {
  id?: string
  name: string
  description: string
  icon: string | null
  category: string
  agentKind?: AgentKind
  invocationMode?: AgentInvocationMode
  visibility?: AgentVisibility
  packageVersion?: string
}

export type ExpertPackageSnapshot = Record<string, unknown>

export interface ConversationExpertBinding {
  id: string
  conversationId: string
  expertId: string
  state: 'active' | 'replaced' | 'removed'
  activationSource: string
  expertVersion?: string
  packageHash?: string
  displaySnapshot: ExpertDisplaySnapshot
  packageSnapshot: ExpertPackageSnapshot
  activatedAt: number
  deactivatedAt?: number | null
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
  traceId?: string | null
  rootSpanId?: string | null
}

export interface ChildRunBudget {
  maxDurationMs: number
  maxTotalTokens: number
  maxOutputTokens: number
  maxToolCalls: number
}

export interface ChildRunRecord {
  id: string
  parentRunId: string
  childRunId: string
  childConversationId: string
  workerAgentId: string
  workerAgentName: string
  objective: string
  context: string
  teamRunId: string | null
  teamMemberId: string | null
  allowedTools: string[] | null
  status: 'queued' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled' | 'interrupted'
  depth: number
  budget: ChildRunBudget
  resultText: string | null
  inputTokens: number
  outputTokens: number
  totalTokens: number
  toolCallCount: number
  errorCode: string | null
  errorMessage: string | null
  createdAt: number
  startedAt: number | null
  finishedAt: number | null
}

export interface ChildRunNotification {
  parentRunId: string
  parentConversationId: string
  childRun: ChildRunRecord
}

export interface ConversationDetail {
  /** Authoritative-only, independently versioned read model; never a Legacy sequence. */
  kernelSnapshot?: KernelRunSnapshot | null
  conversation: ConversationSummary
  messages: ConversationMessage[]
  runtimeEvents: RunEventRecord[]
  toolCalls: ToolCallRecord[]
  approvals: ApprovalRecord[]
  attachments: AttachmentRecord[]
  artifacts: ArtifactRecord[]
  knowledgeBindings: KnowledgeBindingRecord[]
  knowledgeReferences?: KnowledgeReference[]
  expertBindings: ConversationExpertBinding[]
  lastRun: RunRecord | null
  /** Persisted terminal state for runs referenced by the loaded messages. */
  runs?: RunRecord[]
  hasEarlierMessages: boolean
  goals: GoalRecord[]
  tasks: WorkTaskRecord[]
  evidence: TaskEvidenceRecord[]
  planRevisions: PlanRevisionRecord[]
  reviewFindings: ReviewFindingRecord[]
  acceptances: AcceptanceRecord[]
  childRuns: ChildRunRecord[]
  expertWorkflow: ExpertWorkflowSnapshot | null
  expertTeam: ExpertTeamSnapshot | null
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
  /**
   * Host-verified lifecycle bucket: `deliverable`, `preview` or `process`.
   * The renderer groups by this value instead of guessing from the extension,
   * so a CSV the user asked to be delivered stays a deliverable while the same
   * CSV computed as an intermediate step stays a process file.
   */
  artifactClass: string
  /** `project` or `host_private` — where the bytes actually live. */
  artifactOrigin: string
  storagePath: string
  mediaType: string | null
  byteSize: number
  sha256: string | null
  status: string
  createdAt: number
  updatedAt: number
  delivery?: {
    versionId: string | null
    versionNo: number | null
    sourceToolCallId: string | null
    purposeSource: 'user_request' | 'host_rule' | 'unknown'
    verificationStatus: 'passed' | 'limited' | 'failed' | 'stale' | 'unavailable' | 'unverified'
    summary: string
  } | null
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

export interface ApprovalRequest {
  tool?: string
  title?: string
  target?: string
  summary?: string
  diff?: string | null
  command?: string | null
  cwd?: string | null
  /** Untrusted wire value. Parse through the approval decision policy before use. */
  category?: unknown
  /** Untrusted wire value. Parse through the approval decision policy before use. */
  availableDecisions?: unknown
  /** Tool-specific, untrusted approval context. */
  arguments?: unknown
  [key: string]: unknown
}

export interface ApprovalRecord {
  id: string
  toolCallId: string
  runId: string
  conversationId: string
  toolName: string
  status: 'pending' | 'approved' | 'denied' | 'cancelled' | 'expired'
  requestedAction: string
  request: ApprovalRequest
  decision: unknown | null
  requestedAt: number
  resolvedAt: number | null
}

export type ApprovalDecision = 'deny' | 'allow_once' | 'allow_conversation'

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

export interface SetGoalRunningResult {
  goal: GoalRecord
  startedRun: StartRunResult | null
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

export interface WorkEventRecord {
  type: string
  schemaVersion: number
  conversationId: string
  goalId: string | null
  taskId: string | null
  runId: string | null
  traceId: string | null
  spanId: string | null
  sequence: number
  timestamp: string
  data: Record<string, unknown>
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
  icon: string | null
  category: string
  openingSuggestions: string[]
  systemPrompt: string
  isBuiltin: boolean
  packageVersion: string
  packageManifest: ExpertPackageManifest
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

export type ArtifactPreviewKind = 'text' | 'markdown' | 'html' | 'image' | 'pdf' | 'unknown'

/** Host-validated artifact metadata. The renderer intentionally receives no storage path. */
export interface ArtifactGatewayArtifact {
  id: string
  conversationId: string
  runId: string | null
  displayName: string
  artifactType: string
  mediaType: string | null
  byteSize: number
  sha256: string | null
  status: string
  createdAt: number
  updatedAt: number
}

export interface ArtifactCapabilities {
  preview: boolean
  open: boolean
  reveal: boolean
  copyPath: boolean
  openWith: boolean
}

export interface ArtifactApplication {
  id: string
  label: string
}

export interface ArtifactPreview {
  kind: ArtifactPreviewKind
  available: boolean
  content: string | null
  truncated: boolean
  byteSize: number
  reason: string | null
}

export interface ArtifactInspectResponse {
  artifact: ArtifactGatewayArtifact
  capabilities: ArtifactCapabilities
  applications: ArtifactApplication[]
  preview: ArtifactPreview
}

export interface ArtifactActionResponse extends ArtifactInspectResponse {
  action: 'preview' | 'open' | 'open_with' | 'reveal' | 'copy_path'
  completed: boolean
}

export type KnowledgeReference =
  | {
      source: 'local'
      providerKey?: 'local'
      id: string
      revision?: string
    }
  | {
      source: 'remote'
      providerKey?: string
      connectionId: string
      id: string
      revision?: string
    }

export interface ExpertPackageManifest {
  version?: string
  manifestSchemaVersion?: number
  prompt?: string
  skills?: string[]
  knowledge?: string[]
  knowledgeReferences?: KnowledgeReference[]
  mcpServers?: string[]
  allowedTools?: string[]
  [key: string]: unknown
}

export interface SaveAgentInput {
  id?: string
  name: string
  description: string
  icon?: string | null
  category: string
  systemPrompt: string
  defaultModel: string
  openingSuggestions: string[]
  packageManifest: ExpertPackageManifest
}

export interface MissingExpertPackageResources {
  agents: string[]
  skills: string[]
  tools: string[]
  mcpServers: string[]
  knowledgeReferences: KnowledgeReference[]
}

export interface ExpertPackagePreview {
  packageId: string
  name: string
  version: string
  packageHash: string
  action: 'install' | 'upgrade' | 'no_change' | 'downgrade' | 'conflict'
  currentVersion: string | null
  currentHash: string | null
  compatible: boolean
  foxVersion: string
  requestedProjectPermission: 'none' | 'read_only' | 'ask' | 'allow'
  missingResources: MissingExpertPackageResources
  warnings: string[]
  canInstall: boolean
}

export interface ExpertPackageVersionRecord {
  id: string
  expertId: string
  packageId: string
  version: string
  packageHash: string
  package: Record<string, unknown>
  source: 'imported' | 'local' | 'builtin'
  status: 'active' | 'historical'
  createdAt: number
  activatedAt: number
}

export interface ExpertWorkflowRunRecord {
  id: string
  conversationId: string
  expertBindingId: string
  expertId: string
  packageHash: string
  workflowId: string
  workflowVersion: string
  workflow: Record<string, unknown>
  goalId: string
  status: 'running' | 'awaiting_gate' | 'completed' | 'failed' | 'cancelled'
  currentStageIndex: number
  input: unknown
  output: unknown | null
  errorMessage: string | null
  createdAt: number
  updatedAt: number
  completedAt: number | null
}

export interface ExpertWorkflowStageRunRecord {
  id: string
  workflowRunId: string
  stageId: string
  taskId: string
  ordinal: number
  status: 'pending' | 'queued' | 'running' | 'awaiting_gate' | 'completed' | 'failed' | 'skipped'
  attempt: number
  maxAttempts: number
  output: unknown | null
  errorMessage: string | null
  startedAt: number | null
  completedAt: number | null
  updatedAt: number
}

export interface ExpertWorkflowGateRecord {
  id: string
  workflowRunId: string
  stageId: string
  status: 'pending' | 'approved' | 'rejected'
  reason: string
  requestedAt: number
  resolvedAt: number | null
  resolvedBy: string | null
}

export interface ExpertWorkflowSnapshot {
  run: ExpertWorkflowRunRecord
  stages: ExpertWorkflowStageRunRecord[]
  gates: ExpertWorkflowGateRecord[]
}

export interface ExpertTeamRunRecord {
  id: string
  conversationId: string
  expertBindingId: string
  expertId: string
  packageHash: string
  teamId: string
  teamVersion: string
  team: Record<string, unknown>
  parentRunId: string
  status: 'running' | 'completed' | 'failed' | 'cancelled'
  objective: string
  context: string
  result: unknown | null
  errorMessage: string | null
  createdAt: number
  updatedAt: number
  completedAt: number | null
}

export interface ExpertTeamSnapshot {
  run: ExpertTeamRunRecord
  members: ChildRunRecord[]
}

export interface DigitalColleagueRecord {
  id: string
  name: string
  expertId: string
  expertBindingId: string
  packageHash: string
  packageSnapshot: Record<string, unknown>
  conversationId: string
  objective: string
  projectId: string | null
  projectRoot: string | null
  knowledgeReferences: KnowledgeReference[]
  status: 'active' | 'paused' | 'revoked'
  maxRunsPerDay: number
  maxTokensPerDay: number
  maxDurationMs: number
  maxOutputTokens: number
  maxToolCalls: number
  createdAt: number
  updatedAt: number
  revokedAt: number | null
}

export interface DigitalColleagueScheduleRecord {
  id: string
  colleagueId: string
  name: string
  scheduleKind: 'interval'
  intervalSeconds: number
  catchupWindowSeconds: number
  enabled: boolean
  nextDueAt: number
  lastScheduledAt: number | null
  createdAt: number
  updatedAt: number
}

export interface DigitalColleagueChannelRecord {
  id: string
  colleagueId: string
  name: string
  channelKind: 'webhook' | 'im_bridge'
  externalIdentity: string
  secretPrefix: string
  rateLimitPerMinute: number
  status: 'active' | 'revoked'
  createdAt: number
  updatedAt: number
  revokedAt: number | null
}

export interface DigitalColleagueChannelCreated {
  channel: DigitalColleagueChannelRecord
  secret: string
}

export interface DigitalColleagueTriggerRecord {
  id: string
  colleagueId: string
  sourceType: 'manual' | 'schedule' | 'channel'
  sourceId: string | null
  idempotencyKey: string
  status: 'accepted' | 'queued' | 'running' | 'completed' | 'failed' | 'cancelled' | 'rejected' | 'skipped'
  payload: unknown
  scheduledFor: number | null
  runId: string | null
  inputTokens: number
  outputTokens: number
  totalTokens: number
  toolCallCount: number
  errorCode: string | null
  errorMessage: string | null
  createdAt: number
  updatedAt: number
  completedAt: number | null
}

export interface DigitalColleagueAuditRecord {
  id: string
  colleagueId: string | null
  channelId: string | null
  triggerId: string | null
  event: string
  outcome: 'accepted' | 'applied' | 'rejected' | 'skipped' | 'failed'
  actor: string
  details: unknown
  createdAt: number
}

export interface CreateDigitalColleagueInput {
  name: string
  expertId: string
  objective: string
  projectId?: string | null
  projectRoot?: string | null
  knowledgeReferences?: KnowledgeReference[]
  maxRunsPerDay?: number
  maxTokensPerDay?: number
  maxDurationMs?: number
  maxOutputTokens?: number
  maxToolCalls?: number
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

export interface PlanRevisionRecord {
  id: string
  goalId: string
  conversationId: string
  revision: number
  title: string
  summary: string
  tasks: unknown[]
  status: 'proposed' | 'approved' | 'rejected' | 'superseded'
  createdBy: string
  createdAt: string
  approvedAt: string | null
}

export interface ReviewFindingRecord {
  id: string
  goalId: string
  taskId: string | null
  planRevisionId: string | null
  conversationId: string
  severity: 'critical' | 'high' | 'medium' | 'low' | 'info'
  category: string
  title: string
  detail: string
  status: 'open' | 'resolved' | 'waived'
  createdBy: string
  createdAt: string
  resolvedAt: string | null
}

export interface AcceptanceRecord {
  id: string
  goalId: string
  planRevisionId: string | null
  conversationId: string
  status: 'pending' | 'accepted' | 'rejected'
  summary: string
  checks: Record<string, unknown>
  reviewer: string
  createdAt: string
  resolvedAt: string | null
}

export interface WorkStateCounts {
  goals: number
  tasks: number
  evidence: number
  workEvents: number
  runs: number
  runtimeEvents: number
  toolCalls: number
  goalStatuses: Record<string, number>
  taskStatuses: Record<string, number>
  evidenceValidity: Record<string, number>
}

export interface WorkStateFinding {
  severity: 'error' | 'warning'
  code: string
  message: string
  goalId: string | null
  taskId: string | null
  evidenceId: string | null
}

export interface WorkStateDiagnosticReport {
  schemaVersion: number
  eventSchemaVersion: number
  conversationId: string
  checkedAt: string
  healthy: boolean
  counts: WorkStateCounts
  findings: WorkStateFinding[]
}
