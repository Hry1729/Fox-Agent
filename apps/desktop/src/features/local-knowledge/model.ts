import type { KnowledgeReference as SharedKnowledgeReference } from '@/features/conversations/model/types'

export type KnowledgeReference = SharedKnowledgeReference

export type KnowledgeBaseStatus = 'empty' | 'indexing' | 'ready' | 'error' | 'migrating'

export type KnowledgeSearchMode = 'keyword' | 'vector' | 'hybrid'

export interface LocalKnowledgeEmbeddingModel {
  id: string
  name: string
  version: string
  dimension: number
}

export interface LocalKnowledgeBase {
  id: string
  name: string
  description: string
  status: KnowledgeBaseStatus
  documentCount: number
  /** A null count means that the Host has not measured this capability. */
  chunkCount: number | null
  vectorCount: number | null
  activeGeneration: number | null
  textIndexReady: boolean
  vectorIndexReady: boolean
  searchMode: KnowledgeSearchMode
  configuredEmbeddingModelId: string | null
  chunkSize: number
  chunkOverlap: number
  embeddingModel: LocalKnowledgeEmbeddingModel | null
  fallbackReason: string | null
  storagePath: string
  writable: boolean
  lastIndexedAt: string | null
  updatedAt: string
}

export interface LocalKnowledgeBaseDetail extends LocalKnowledgeBase {
  parserVersion: string | null
  storage: {
    freeBytes: number | null
    totalBytes: number | null
  }
  recentJobs: KnowledgeJob[]
}

export interface LocalKnowledgeStorageStatus {
  rootPath: string
  databasePath: string
  writable: boolean
  schemaVersion: number
  pendingRootPath: string | null
  restartRequired: boolean
  migrationId?: string | null
}

export type StorageMigrationStatus = 'queued' | 'running' | 'paused' | 'completed' | 'failed' | 'cancelled' | 'interrupted'

export type StorageMigrationStage =
  | 'validating'
  | 'quiescing'
  | 'flushing'
  | 'closing_handles'
  | 'copying'
  | 'verifying'
  | 'switching'
  | 'reopening'
  | 'self_checking'
  | 'awaiting_cleanup'
  | 'rolling_back'

export interface LocalKnowledgeStorageMigration {
  id: string
  sourcePath: string
  destinationPath: string
  status: StorageMigrationStatus
  stage: StorageMigrationStage | null
  progress: number
  lastSequence: number
  error: OperationError | null
  createdAt: string
  updatedAt: string
}

export type KnowledgeDocumentStatus = 'queued' | 'parsing' | 'indexed' | 'error' | 'stale'

export interface LocalKnowledgeDocument {
  id: string
  knowledgeBaseId: string
  name: string
  extension: string
  mimeType: string
  relativePath: string
  sizeBytes: number
  status: KnowledgeDocumentStatus
  chunkCount: number
  sourceRevision: string | null
  parseError: string | null
  importedAt: string
  updatedAt: string
}

export interface LocalKnowledgeImportFile {
  name: string
  sourcePath?: string
  relativePath?: string
  mimeType: string
  sizeBytes: number
}

export interface LocalKnowledgeFolder {
  knowledgeBaseId: string
  name: string
  relativePath: string
  documentCount: number
}

export type LocalFileCategory = 'all' | 'text' | 'document' | 'pdf' | 'presentation' | 'spreadsheet'

export interface LocalKnowledgeFileSource {
  id: string
  rootPath: string
  displayName: string
  fileCount: number
  totalSize: number
  lastScannedAt: string | null
  createdAt: string
  updatedAt: string
}

export interface LocalKnowledgeCatalogFile {
  id: string
  sourceId: string
  sourceName: string
  sourcePath: string
  absolutePath: string
  relativePath: string
  name: string
  extension: string
  mimeType: string
  sizeBytes: number
  modifiedAt: string | null
}

export interface LocalKnowledgeImportRequest {
  knowledgeBaseId: string
  files: LocalKnowledgeImportFile[]
  parserVersion?: string
  chunkConfigHash?: string
}

export type KnowledgeOperationStage =
  | 'validating'
  | 'copying'
  | 'hashing'
  | 'parsing'
  | 'chunking'
  | 'embedding'
  | 'vector_upsert'
  | 'verifying'
  | 'committing'

export type OperationStatus = 'queued' | 'running' | 'paused' | 'completed' | 'failed' | 'cancelled'

export interface OperationData {
  completedItems?: number
  succeededItems?: number
  failedItems?: number
  totalItems?: number
  processedBytes?: number
  totalBytes?: number
  resultEntityId?: string
  outcome?: 'success' | 'partial'
  [key: string]: unknown
}

export interface OperationError {
  code: string
  message: string
  retryable: boolean
}

export interface OperationEvent {
  schemaVersion: number
  operationId: string
  parentOperationId?: string
  sequence: number
  domain: 'knowledge'
  entityId?: string
  operation: string
  status: OperationStatus
  stage?: KnowledgeOperationStage
  progress?: number
  data?: OperationData
  error?: OperationError
  occurredAt: string
}

export type KnowledgeJobType = 'import' | 'parse' | 'index' | 'delete' | 'rebuild'
export type KnowledgeJobStatus = OperationStatus | 'interrupted'

export interface KnowledgeJob {
  id: string
  operationId: string
  knowledgeBaseId: string
  type: KnowledgeJobType
  status: KnowledgeJobStatus
  stage: KnowledgeOperationStage | null
  progress: number
  lastSequence: number
  outcome: 'success' | 'partial' | null
  completedItems: number
  failedItems: number
  totalItems: number
  heartbeatAt: string | null
  error: OperationError | null
  createdAt: string
  updatedAt: string
}

export interface Page<T> {
  items: T[]
  nextCursor?: string
  total: number
}

export interface OperationAccepted {
  operationId: string
  acceptedAt: string
}

export interface OperationSnapshot {
  operationId: string
  lastSequence: number
  lastEvent: OperationEvent | null
}

export interface LocalKnowledgeBaseCreateRequest {
  name: string
  description?: string | null
  embeddingModelId?: string | null
  chunkSize?: number
  chunkOverlap?: number
  searchMode?: KnowledgeSearchMode
}

export type EmbeddingModelStatus = 'not_installed' | 'installing' | 'ready' | 'error'
export type EmbeddingIntegrityStatus = 'pending' | 'verifying' | 'verified' | 'failed'

export interface EmbeddingModelAsset {
  name: string
  downloadName: string
  url: string
  sha256: string
  sizeBytes: number
}

export interface EmbeddingModelDownload {
  id: string
  status: 'queued' | 'running' | 'paused' | 'completed' | 'failed' | 'cancelled'
  progress: number
  downloadedBytes: number
  totalBytes: number
  currentFile: string | null
  errorCode: string | null
  errorMessage: string | null
  updatedAt: number
}

export interface LocalEmbeddingModel {
  id: string
  name: string
  version: string
  languages: string[]
  dimension: number
  license: string
  sourceUrl: string
  status: EmbeddingModelStatus
  integrityStatus: EmbeddingIntegrityStatus
  isDefault: boolean
  recommended: boolean
  sizeBytes: number
  installedAt: number | null
  packagePath: string | null
  loadReady: boolean
  lastErrorCode: string | null
  lastErrorMessage: string | null
  files: EmbeddingModelAsset[]
  download: EmbeddingModelDownload | null
}

export interface EmbeddingModelTestResult {
  integrityVerified: boolean
  loadReady: boolean
  dimension: number
  elapsedMs: number
  message: string
  errorCode: string | null
  errorDetails: string | null
}

export interface VectorBackendHealth {
  backend: string
  available: boolean
  readWriteVerified: boolean
  fallbackActive: boolean
  message: string
}

export interface KnowledgeRetrievalResult {
  knowledgeBaseId: string
  query: string
  mode: KnowledgeSearchMode
  fusionVersion: string | null
  timings: { keywordMs: number; vectorMs: number; totalMs: number }
  items: Array<{
    documentId: string
    documentName: string
    relativePath: string
    chunkId: string
    anchor: string | null
    content: string
    score: number
    keywordScore: number | null
    vectorScore: number | null
  }>
}

export interface KnowledgeRetrievalCase {
  id: string
  knowledgeBaseId: string
  question: string
  expectedDocumentIds: string[]
  expectedKeywords: string[]
  createdAt: number
  updatedAt: number
}

export interface KnowledgeChunkPreview {
  id: string
  chunkIndex: number
  content: string
  anchor: string | null
  startOffset: number | null
  endOffset: number | null
}

export interface LocalKnowledgeGateway {
  listKnowledgeBases(): Promise<LocalKnowledgeBase[]>
  listFileSources(): Promise<LocalKnowledgeFileSource[]>
  pickFileSourceFolder(): Promise<string | null>
  addFileSource(path: string): Promise<LocalKnowledgeFileSource>
  rescanFileSource(id: string): Promise<LocalKnowledgeFileSource>
  updateFileSource(id: string, displayName: string): Promise<LocalKnowledgeFileSource>
  removeFileSource(id: string): Promise<boolean>
  listLocalFiles(options?: { sourceId?: string; query?: string; category?: LocalFileCategory; limit?: number; offset?: number }): Promise<LocalKnowledgeCatalogFile[]>
  readLocalFileRange(id: string, start: number, end: number): Promise<unknown>
  openLocalFile(id: string): Promise<boolean>
  revealLocalFile(id: string): Promise<boolean>
  getKnowledgeBase(id: string): Promise<LocalKnowledgeBaseDetail>
  createKnowledgeBase(request: LocalKnowledgeBaseCreateRequest): Promise<LocalKnowledgeBase>
  updateKnowledgeBase(id: string, request: LocalKnowledgeBaseCreateRequest): Promise<LocalKnowledgeBase>
  deleteKnowledgeBase(id: string): Promise<boolean>
  listDocuments(id: string, options?: { query?: string }): Promise<Page<LocalKnowledgeDocument>>
  listFolders(id: string): Promise<LocalKnowledgeFolder[]>
  createFolder(knowledgeBaseId: string, name: string, parentPath?: string): Promise<LocalKnowledgeFolder>
  readDocumentFileRange(knowledgeBaseId: string, documentId: string, start: number, end: number): Promise<unknown>
  openDocumentFile(knowledgeBaseId: string, documentId: string): Promise<boolean>
  revealDocumentFile(knowledgeBaseId: string, documentId: string): Promise<boolean>
  pickImportFiles?(): Promise<LocalKnowledgeImportFile[]>
  pickImportFolder?(): Promise<LocalKnowledgeImportFile[]>
  startImport(request: LocalKnowledgeImportRequest): Promise<OperationAccepted>
  listJobs(knowledgeBaseId?: string): Promise<KnowledgeJob[]>
  getOperation(operationId: string): Promise<OperationSnapshot>
  subscribeOperation(operationId: string, listener: (event: OperationEvent) => void): () => void
  cancelJob(jobId: string): Promise<KnowledgeJob>
  retryJob(jobId: string): Promise<OperationAccepted>
  getStorageStatus(): Promise<LocalKnowledgeStorageStatus>
  pickStorageDirectory(): Promise<string | null>
  startStorageMigration(destinationPath: string): Promise<OperationAccepted>
  getStorageMigration(migrationId: string): Promise<LocalKnowledgeStorageMigration>
  listEmbeddingModels(): Promise<LocalEmbeddingModel[]>
  startEmbeddingModelInstall(modelId: string): Promise<OperationAccepted>
  cancelEmbeddingModelDownload(downloadId: string): Promise<boolean>
  retryEmbeddingModelDownload(downloadId: string): Promise<OperationAccepted>
  testEmbeddingModel(modelId: string): Promise<EmbeddingModelTestResult>
  setDefaultEmbeddingModel(modelId: string): Promise<boolean>
  deleteEmbeddingModel(modelId: string): Promise<boolean>
  importEmbeddingModelPackage(packagePath: string): Promise<LocalEmbeddingModel>
  getVectorBackendHealth(): Promise<VectorBackendHealth>
  startIndex(knowledgeBaseId: string, rebuild?: boolean): Promise<OperationAccepted>
  testRetrieval(knowledgeBaseId: string, query: string, mode: KnowledgeSearchMode, limit?: number): Promise<KnowledgeRetrievalResult>
  listRetrievalCases(knowledgeBaseId: string): Promise<KnowledgeRetrievalCase[]>
  saveRetrievalCase(request: { id?: string; knowledgeBaseId: string; question: string; expectedDocumentIds?: string[]; expectedKeywords?: string[] }): Promise<KnowledgeRetrievalCase>
  deleteRetrievalCase(id: string): Promise<boolean>
  exportRetrievalCases(knowledgeBaseId: string): Promise<{ json: unknown; markdown: string }>
  listDocumentChunks(knowledgeBaseId: string, documentId: string, options?: { limit?: number; offset?: number }): Promise<KnowledgeChunkPreview[]>
  deleteDocument(knowledgeBaseId: string, documentId: string): Promise<boolean>
  reparseDocument(knowledgeBaseId: string, documentId: string): Promise<OperationAccepted>
}

export function knowledgeReferenceLabel(reference: KnowledgeReference): string {
  return reference.source === 'local' ? `本地知识库 ${reference.id}` : `远程知识库 ${reference.id}`
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`
}

export function formatJobStage(stage: KnowledgeOperationStage | null): string {
  const labels: Record<KnowledgeOperationStage, string> = {
    validating: '校验文件',
    copying: '复制文件',
    hashing: '计算哈希',
    parsing: '解析内容',
    chunking: '切分文本',
    embedding: '生成向量',
    vector_upsert: '写入向量索引',
    verifying: '校验索引',
    committing: '提交代次',
  }
  return stage ? labels[stage] : '等待开始'
}
