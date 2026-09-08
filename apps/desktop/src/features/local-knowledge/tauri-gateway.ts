import {
  desktopClient,
  desktopRuntimeAvailable,
  type LocalKnowledgeBaseDto,
  type LocalKnowledgeCatalogFileDto,
  type LocalKnowledgeDocumentDto,
  type LocalKnowledgeFileSourceDto,
  type LocalKnowledgeFolderDto,
  type LocalKnowledgeJobDto,
  type LocalKnowledgeOperationAcceptedDto,
  type LocalKnowledgeStorageDirectoryDto,
  type LocalKnowledgeStorageMigrationDto,
  type PickedLocalKnowledgeFileDto,
  type LocalKnowledgeStorageStatusDto,
  type LocalEmbeddingModelDto,
} from '@/features/conversations/api/desktop-client'
import { mockLocalKnowledgeGateway } from './mock-gateway'
import type {
  KnowledgeJob,
  KnowledgeJobStatus,
  KnowledgeOperationStage,
  LocalKnowledgeBase,
  LocalKnowledgeBaseDetail,
  LocalKnowledgeBaseCreateRequest,
  KnowledgeSearchMode,
  LocalKnowledgeDocument,
  LocalKnowledgeCatalogFile,
  LocalKnowledgeFileSource,
  LocalKnowledgeFolder,
  LocalKnowledgeGateway,
  LocalKnowledgeImportRequest,
  LocalKnowledgeStorageMigration,
  LocalKnowledgeStorageStatus,
  StorageMigrationStage,
  StorageMigrationStatus,
  OperationAccepted,
  OperationEvent,
  OperationSnapshot,
  Page,
  LocalEmbeddingModel,
  KnowledgeRetrievalCase,
  KnowledgeRetrievalResult,
  KnowledgeChunkPreview,
} from './model'

export class LocalKnowledgeGatewayError extends Error {
  readonly code: string
  readonly retryable: boolean

  constructor(code: string, message: string, retryable = false) {
    super(message)
    this.name = 'LocalKnowledgeGatewayError'
    this.code = code
    this.retryable = retryable
  }
}

export type LocalKnowledgeDesktopClient = Pick<
  typeof desktopClient,
  | 'listLocalKnowledgeBases'
  | 'listLocalKnowledgeFileSources'
  | 'pickLocalKnowledgeSourceFolder'
  | 'addLocalKnowledgeFileSource'
  | 'rescanLocalKnowledgeFileSource'
  | 'updateLocalKnowledgeFileSource'
  | 'removeLocalKnowledgeFileSource'
  | 'listLocalKnowledgeLocalFiles'
  | 'readLocalKnowledgeLocalFileRange'
  | 'openLocalKnowledgeLocalFile'
  | 'getLocalKnowledgeBase'
  | 'createLocalKnowledgeBase'
  | 'updateLocalKnowledgeBase'
  | 'deleteLocalKnowledgeBase'
  | 'listLocalKnowledgeDocuments'
  | 'listLocalKnowledgeFolders'
  | 'createLocalKnowledgeFolder'
  | 'readLocalKnowledgeDocumentFileRange'
  | 'openLocalKnowledgeDocumentFile'
  | 'pickLocalKnowledgeImportFiles'
  | 'pickLocalKnowledgeImportFolder'
  | 'startLocalKnowledgeImport'
  | 'listLocalKnowledgeJobs'
  | 'getLocalKnowledgeJob'
  | 'cancelLocalKnowledgeJob'
  | 'retryLocalKnowledgeJob'
  | 'getLocalKnowledgeStorageStatus'
  | 'pickLocalKnowledgeStorageDirectory'
  | 'startLocalKnowledgeStorageMigration'
  | 'getLocalKnowledgeStorageMigration'
  | 'listLocalEmbeddingModels'
  | 'startLocalEmbeddingModelInstall'
  | 'cancelLocalEmbeddingModelDownload'
  | 'retryLocalEmbeddingModelDownload'
  | 'testLocalEmbeddingModel'
  | 'setDefaultLocalEmbeddingModel'
  | 'deleteLocalEmbeddingModel'
  | 'importLocalEmbeddingModelPackage'
  | 'getLocalVectorBackendHealth'
  | 'startLocalKnowledgeIndex'
  | 'testLocalKnowledgeRetrieval'
  | 'listLocalKnowledgeRetrievalCases'
  | 'saveLocalKnowledgeRetrievalCase'
  | 'deleteLocalKnowledgeRetrievalCase'
  | 'exportLocalKnowledgeRetrievalCases'
  | 'listLocalKnowledgeDocumentChunks'
  | 'deleteLocalKnowledgeDocument'
  | 'reparseLocalKnowledgeDocument'
>

const operationStages: ReadonlySet<string> = new Set([
  'validating',
  'copying',
  'hashing',
  'parsing',
  'chunking',
  'embedding',
  'vector_upsert',
  'verifying',
  'committing',
])

const operationStatuses: ReadonlySet<string> = new Set([
  'queued',
  'running',
  'paused',
  'completed',
  'failed',
  'cancelled',
  'interrupted',
])

const storageMigrationStages: ReadonlySet<string> = new Set([
  'validating',
  'quiescing',
  'flushing',
  'closing_handles',
  'copying',
  'verifying',
  'switching',
  'reopening',
  'self_checking',
  'awaiting_cleanup',
  'rolling_back',
])

const storageMigrationStatuses: ReadonlySet<string> = new Set([
  'queued',
  'running',
  'paused',
  'completed',
  'failed',
  'cancelled',
  'interrupted',
])

function toIsoTimestamp(value: number | null | undefined): string | null {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) return null
  const milliseconds = value < 100_000_000_000 ? value * 1000 : value
  const timestamp = new Date(milliseconds)
  return Number.isNaN(timestamp.getTime()) ? null : timestamp.toISOString()
}

function requiredIsoTimestamp(value: number): string {
  return toIsoTimestamp(value) ?? ''
}

function numberValue(value: number | null | undefined): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function parseGenerationSequence(value: string | null): number | null {
  if (!value) return null
  const match = /^(?:g|generation[-_:]?)(\d+)$/i.exec(value) ?? /^(\d+)$/.exec(value)
  if (!match) return null
  const sequence = Number(match[1])
  return Number.isSafeInteger(sequence) ? sequence : null
}

function baseStatus(dto: LocalKnowledgeBaseDto): LocalKnowledgeBase['status'] {
  const activeJobStatus = dto.activeJobStatus?.toLowerCase()
  if (activeJobStatus === 'failed' || activeJobStatus === 'error' || activeJobStatus === 'interrupted') return 'error'
  if (activeJobStatus === 'queued' || activeJobStatus === 'running' || activeJobStatus === 'paused') return 'indexing'
  if (dto.textIndexReady === true || dto.vectorIndexReady === true) return 'ready'
  return dto.documentCount > 0 ? 'error' : 'empty'
}

function searchMode(value: string | null | undefined): KnowledgeSearchMode {
  return value === 'vector' || value === 'hybrid' ? value : 'keyword'
}

function nullableCount(value: number | null | undefined): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? Math.trunc(value) : null
}

export function mapLocalKnowledgeBase(dto: LocalKnowledgeBaseDto, storage?: LocalKnowledgeStorageStatusDto): LocalKnowledgeBase {
  const vectorIndexReady = dto.vectorIndexReady === true
  const vectorCapabilityReported = 'vectorIndexReady' in dto
  const embeddingModel = dto.embeddingModel ?? null
  return {
    id: dto.id,
    name: dto.name,
    description: dto.description ?? '',
    status: baseStatus(dto),
    documentCount: Math.max(0, numberValue(dto.documentCount)),
    chunkCount: nullableCount(dto.chunkCount),
    vectorCount: nullableCount(dto.vectorCount),
    activeGeneration: parseGenerationSequence(dto.activeIndexGeneration),
    textIndexReady: dto.textIndexReady === true,
    vectorIndexReady,
    searchMode: searchMode(dto.searchMode),
    configuredEmbeddingModelId: dto.configuredEmbeddingModelId ?? null,
    chunkSize: Math.max(128, Math.trunc(numberValue(dto.chunkSize) || 512)),
    chunkOverlap: Math.max(0, Math.trunc(numberValue(dto.chunkOverlap) || 50)),
    embeddingModel,
    fallbackReason: dto.fallbackReason ?? (vectorIndexReady ? null : vectorCapabilityReported ? '向量索引未就绪' : 'Host 未提供向量索引状态'),
    storagePath: storage?.rootPath ?? '',
    writable: storage?.writable ?? false,
    lastIndexedAt: toIsoTimestamp(dto.lastIndexedAt),
    updatedAt: requiredIsoTimestamp(dto.updatedAt),
  }
}

export function mapLocalKnowledgeDetail(
  dto: LocalKnowledgeBaseDto,
  storage: LocalKnowledgeStorageStatusDto,
  recentJobs: KnowledgeJob[] = [],
): LocalKnowledgeBaseDetail {
  return {
    ...mapLocalKnowledgeBase(dto, storage),
    parserVersion: dto.parserVersion ?? null,
    storage: {
      freeBytes: nullableCount(dto.storageFreeBytes),
      totalBytes: nullableCount(dto.storageTotalBytes),
    },
    recentJobs: recentJobs.slice(0, 3),
  }
}

export function mapLocalKnowledgeStorageStatus(dto: LocalKnowledgeStorageStatusDto): LocalKnowledgeStorageStatus {
  return {
    rootPath: dto.rootPath,
    databasePath: dto.databasePath,
    writable: dto.writable,
    schemaVersion: Math.max(0, Math.trunc(numberValue(dto.schemaVersion))),
    pendingRootPath: dto.pendingRootPath ?? null,
    restartRequired: dto.restartRequired === true,
    migrationId: dto.migrationId ?? null,
  }
}

function documentStatus(dto: LocalKnowledgeDocumentDto): LocalKnowledgeDocument['status'] {
  const parseStatus = dto.parseStatus.toLowerCase()
  const indexStatus = dto.indexStatus.toLowerCase()
  if (dto.lastErrorCode || dto.lastErrorMessage || ['error', 'failed', 'failure'].includes(parseStatus) || ['error', 'failed', 'failure'].includes(indexStatus)) return 'error'
  if (['stale', 'outdated'].includes(indexStatus)) return 'stale'
  if (['indexed', 'ready', 'complete', 'completed'].includes(indexStatus)) return 'indexed'
  if (['parsing', 'parsed', 'chunking', 'embedding', 'indexing'].includes(parseStatus) || ['parsing', 'indexing', 'embedding'].includes(indexStatus)) return 'parsing'
  return 'queued'
}

function fileExtension(name: string): string {
  const match = /\.([^.\\/]+)$/.exec(name)
  return match ? match[1].toLowerCase() : 'file'
}

export function mapLocalKnowledgeDocument(dto: LocalKnowledgeDocumentDto): LocalKnowledgeDocument {
  return {
    id: dto.id,
    knowledgeBaseId: dto.knowledgeBaseId,
    name: dto.displayName,
    extension: fileExtension(dto.displayName || dto.relativePath),
    mimeType: dto.mimeType,
    relativePath: dto.relativePath,
    sizeBytes: Math.max(0, numberValue(dto.fileSize)),
    status: documentStatus(dto),
    chunkCount: Math.max(0, numberValue(dto.chunkCount)),
    sourceRevision: String(dto.currentRevision),
    parseError: dto.lastErrorMessage ?? dto.lastErrorCode,
    importedAt: requiredIsoTimestamp(dto.createdAt),
    updatedAt: requiredIsoTimestamp(dto.updatedAt),
  }
}

export function mapLocalKnowledgeFileSource(dto: LocalKnowledgeFileSourceDto): LocalKnowledgeFileSource {
  return {
    id: dto.id,
    rootPath: dto.rootPath,
    displayName: dto.displayName,
    fileCount: Math.max(0, numberValue(dto.fileCount)),
    totalSize: Math.max(0, numberValue(dto.totalSize)),
    lastScannedAt: toIsoTimestamp(dto.lastScannedAt),
    createdAt: requiredIsoTimestamp(dto.createdAt),
    updatedAt: requiredIsoTimestamp(dto.updatedAt),
  }
}

export function mapLocalKnowledgeCatalogFile(dto: LocalKnowledgeCatalogFileDto): LocalKnowledgeCatalogFile {
  return {
    id: dto.id,
    sourceId: dto.sourceId,
    sourceName: dto.sourceName,
    sourcePath: dto.sourcePath,
    absolutePath: dto.absolutePath,
    relativePath: dto.relativePath,
    name: dto.displayName,
    extension: dto.extension,
    mimeType: dto.mimeType,
    sizeBytes: Math.max(0, numberValue(dto.fileSize)),
    modifiedAt: toIsoTimestamp(dto.modifiedAt),
  }
}

function jobType(value: string): KnowledgeJob['type'] {
  if (value === 'index' || value === 'rebuild' || value === 'delete') return value
  return 'import'
}

function jobStatus(value: string): KnowledgeJobStatus {
  return operationStatuses.has(value) ? value as KnowledgeJobStatus : 'failed'
}

function jobStage(value: string | null): KnowledgeOperationStage | null {
  return value && operationStages.has(value) ? value as KnowledgeOperationStage : null
}

function jobOutcome(value: string | null): KnowledgeJob['outcome'] {
  return value === 'success' || value === 'partial' ? value : null
}

function storageMigrationStatus(value: string): StorageMigrationStatus {
  return storageMigrationStatuses.has(value) ? value as StorageMigrationStatus : 'failed'
}

function storageMigrationStage(value: string | null): StorageMigrationStage | null {
  return value && storageMigrationStages.has(value) ? value as StorageMigrationStage : null
}

export function mapLocalKnowledgeStorageMigration(dto: LocalKnowledgeStorageMigrationDto): LocalKnowledgeStorageMigration {
  const hasError = Boolean(dto.errorCode || dto.errorMessage)
  return {
    id: dto.id,
    sourcePath: dto.sourcePath,
    destinationPath: dto.destinationPath,
    status: storageMigrationStatus(dto.status),
    stage: storageMigrationStage(dto.stage),
    progress: Math.max(0, Math.min(100, Math.trunc(numberValue(dto.progress)))),
    lastSequence: Math.max(0, Math.trunc(numberValue(dto.lastSequence))),
    error: hasError
      ? { code: dto.errorCode ?? 'local_knowledge.migration_failed', message: dto.errorMessage ?? '本地知识库存储迁移失败。', retryable: false }
      : null,
    createdAt: requiredIsoTimestamp(dto.createdAt),
    updatedAt: requiredIsoTimestamp(dto.updatedAt),
  }
}

export function mapLocalKnowledgeJob(dto: LocalKnowledgeJobDto): KnowledgeJob {
  const status = jobStatus(dto.status)
  const hasError = Boolean(dto.errorCode || dto.errorMessage)
  return {
    id: dto.id,
    operationId: dto.parentOperationId ?? dto.id,
    knowledgeBaseId: dto.knowledgeBaseId,
    type: jobType(dto.jobType),
    status,
    stage: jobStage(dto.stage),
    progress: Math.max(0, Math.min(100, Math.trunc(numberValue(dto.progress)))),
    lastSequence: Math.max(0, Math.trunc(numberValue(dto.lastSequence))),
    outcome: jobOutcome(dto.outcome),
    completedItems: 0,
    failedItems: 0,
    totalItems: 0,
    heartbeatAt: toIsoTimestamp(dto.heartbeatAt),
    error: hasError
      ? { code: dto.errorCode ?? 'local_knowledge.job_failed', message: dto.errorMessage ?? '本地知识库任务失败', retryable: false }
      : null,
    createdAt: requiredIsoTimestamp(dto.createdAt),
    updatedAt: requiredIsoTimestamp(dto.updatedAt),
  }
}

function unavailable(code: string, message: string): never {
  throw new LocalKnowledgeGatewayError(code, message, false)
}

function mapPickedFile(dto: PickedLocalKnowledgeFileDto) {
  return {
    name: dto.name,
    sourcePath: dto.sourcePath,
    mimeType: dto.mimeType,
    sizeBytes: Math.max(0, numberValue(dto.sizeBytes)),
    relativePath: dto.relativePath,
  }
}

function mapFolder(dto: LocalKnowledgeFolderDto): LocalKnowledgeFolder {
  return {
    knowledgeBaseId: dto.knowledgeBaseId,
    name: dto.name,
    relativePath: dto.relativePath,
    documentCount: Math.max(0, numberValue(dto.documentCount)),
  }
}

function mapPickedStorageDirectory(dto: LocalKnowledgeStorageDirectoryDto | string | null): string | null {
  if (typeof dto === 'string') return dto.trim() || null
  return dto && typeof dto.path === 'string' && dto.path.trim() ? dto.path : null
}

function mapAccepted(dto: LocalKnowledgeOperationAcceptedDto): OperationAccepted {
  return { operationId: dto.operationId, acceptedAt: requiredIsoTimestamp(dto.acceptedAt) }
}

function mapEmbeddingModel(dto: LocalEmbeddingModelDto): LocalEmbeddingModel {
  const status: LocalEmbeddingModel['status'] = dto.status === 'installing' || dto.status === 'ready' || dto.status === 'error' ? dto.status : 'not_installed'
  const integrityStatus: LocalEmbeddingModel['integrityStatus'] = dto.integrityStatus === 'verifying' || dto.integrityStatus === 'verified' || dto.integrityStatus === 'failed' ? dto.integrityStatus : 'pending'
  const downloadStatus = dto.download?.status
  return {
    ...dto,
    status,
    integrityStatus,
    languages: Array.isArray(dto.languages) ? dto.languages : [],
    files: Array.isArray(dto.files) ? dto.files : [],
    download: dto.download ? {
      ...dto.download,
      status: downloadStatus === 'running' || downloadStatus === 'paused' || downloadStatus === 'completed' || downloadStatus === 'failed' || downloadStatus === 'cancelled' ? downloadStatus : 'queued',
    } : null,
  }
}

function operationEvent(operationId: string, jobs: KnowledgeJob[]): OperationEvent {
  if (jobs.length === 0) unavailable('local_knowledge.operation_not_found', '未找到本地知识库操作。')
  const completedItems = jobs.filter((job) => job.status === 'completed').length
  const failedJobs = jobs.filter((job) => job.status === 'failed' || job.status === 'interrupted')
  const cancelledItems = jobs.filter((job) => job.status === 'cancelled').length
  const hasPartialOutcome = completedItems > 0 && failedJobs.length + cancelledItems > 0
  const status: OperationEvent['status'] = jobs.some((job) => job.status === 'running')
    ? 'running'
    : jobs.some((job) => job.status === 'paused')
      ? 'paused'
      : jobs.some((job) => job.status === 'queued')
        ? 'queued'
        : hasPartialOutcome
          ? 'completed'
          : failedJobs.length > 0
            ? 'failed'
            : cancelledItems === jobs.length
              ? 'cancelled'
              : 'completed'
  const latest = jobs.reduce((current, job) => current.updatedAt > job.updatedAt ? current : job)
  const active = jobs.find((job) => job.status === 'running' || job.status === 'queued' || job.status === 'paused') ?? latest
  const firstError = failedJobs.find((job) => job.error)?.error ?? null
  return {
    schemaVersion: 1,
    operationId,
    sequence: jobs.reduce((total, job) => total + job.lastSequence, 0),
    domain: 'knowledge',
    entityId: latest.knowledgeBaseId,
    operation: latest.type,
    status,
    stage: active.stage ?? undefined,
    progress: Math.round(jobs.reduce((total, job) => total + job.progress, 0) / jobs.length),
    data: {
      completedItems,
      succeededItems: completedItems,
      failedItems: failedJobs.length + cancelledItems,
      totalItems: jobs.length,
      outcome: hasPartialOutcome ? 'partial' : status === 'completed' ? 'success' : undefined,
    },
    error: firstError ?? undefined,
    occurredAt: latest.updatedAt,
  }
}

export function createTauriLocalKnowledgeGateway(client: LocalKnowledgeDesktopClient = desktopClient): LocalKnowledgeGateway {
  return {
    async listKnowledgeBases() {
      const [bases, storage] = await Promise.all([
        client.listLocalKnowledgeBases(),
        client.getLocalKnowledgeStorageStatus(),
      ])
      return bases.map((base) => mapLocalKnowledgeBase(base, storage))
    },
    async listFileSources() {
      return (await client.listLocalKnowledgeFileSources()).map(mapLocalKnowledgeFileSource)
    },
    async pickFileSourceFolder() {
      const path = await client.pickLocalKnowledgeSourceFolder()
      return typeof path === 'string' && path.trim() ? path : null
    },
    async addFileSource(path) {
      const normalizedPath = path.trim()
      if (!normalizedPath) unavailable('local_knowledge.source_path_missing', '请选择需要管理的本地文件夹。')
      return mapLocalKnowledgeFileSource(await client.addLocalKnowledgeFileSource(normalizedPath))
    },
    async rescanFileSource(id) {
      return mapLocalKnowledgeFileSource(await client.rescanLocalKnowledgeFileSource(id))
    },
    async updateFileSource(id, displayName) {
      const normalizedName = displayName.trim()
      if (!normalizedName) unavailable('local_knowledge.source_name_required', '请输入文件夹显示名称。')
      return mapLocalKnowledgeFileSource(await client.updateLocalKnowledgeFileSource(id, normalizedName))
    },
    async removeFileSource(id) {
      return client.removeLocalKnowledgeFileSource(id)
    },
    async listLocalFiles(options = {}) {
      const files = await client.listLocalKnowledgeLocalFiles({
        sourceId: options.sourceId,
        query: options.query,
        category: options.category === 'all' ? undefined : options.category,
        limit: options.limit,
        offset: options.offset,
      })
      return files.map(mapLocalKnowledgeCatalogFile)
    },
    async readLocalFileRange(id, start, end) {
      return client.readLocalKnowledgeLocalFileRange(id, start, end)
    },
    async openLocalFile(id) {
      return client.openLocalKnowledgeLocalFile(id, false)
    },
    async revealLocalFile(id) {
      return client.openLocalKnowledgeLocalFile(id, true)
    },
    async getKnowledgeBase(id) {
      const [base, storage, jobs] = await Promise.all([
        client.getLocalKnowledgeBase(id),
        client.getLocalKnowledgeStorageStatus(),
        client.listLocalKnowledgeJobs(id),
      ])
      return mapLocalKnowledgeDetail(base, storage, jobs.map(mapLocalKnowledgeJob))
    },
    async createKnowledgeBase(request: LocalKnowledgeBaseCreateRequest) {
      const name = request.name.trim()
      if (!name) unavailable('local_knowledge.name_required', '知识库名称不能为空。')
      const description = request.description?.trim() || null
      return mapLocalKnowledgeBase(await client.createLocalKnowledgeBase({
        name,
        description,
        embeddingModelId: request.embeddingModelId,
        chunkSize: request.chunkSize,
        chunkOverlap: request.chunkOverlap,
        searchMode: request.searchMode,
      }))
    },
    async updateKnowledgeBase(id, request) {
      const name = request.name.trim()
      if (!name) unavailable('local_knowledge.name_required', '知识库名称不能为空。')
      const description = request.description?.trim() || null
      return mapLocalKnowledgeBase(await client.updateLocalKnowledgeBase({
        id,
        name,
        description,
        embeddingModelId: request.embeddingModelId,
        chunkSize: request.chunkSize,
        chunkOverlap: request.chunkOverlap,
        searchMode: request.searchMode,
      }))
    },
    async deleteKnowledgeBase(id) {
      return client.deleteLocalKnowledgeBase(id)
    },
    async listDocuments(id, options) {
      const items = await client.listLocalKnowledgeDocuments(id, options?.query)
      return { items: items.map(mapLocalKnowledgeDocument), total: items.length } satisfies Page<LocalKnowledgeDocument>
    },
    async listFolders(id) {
      return (await client.listLocalKnowledgeFolders(id)).map(mapFolder)
    },
    async createFolder(knowledgeBaseId, name, parentPath) {
      const normalizedName = name.trim()
      if (!normalizedName) unavailable('local_knowledge.folder_name_required', '文件夹名称不能为空。')
      return mapFolder(await client.createLocalKnowledgeFolder(knowledgeBaseId, normalizedName, parentPath))
    },
    async readDocumentFileRange(knowledgeBaseId, documentId, start, end) {
      return client.readLocalKnowledgeDocumentFileRange(knowledgeBaseId, documentId, start, end)
    },
    async openDocumentFile(knowledgeBaseId, documentId) {
      return client.openLocalKnowledgeDocumentFile(knowledgeBaseId, documentId, false)
    },
    async revealDocumentFile(knowledgeBaseId, documentId) {
      return client.openLocalKnowledgeDocumentFile(knowledgeBaseId, documentId, true)
    },
    async pickImportFiles() {
      return (await client.pickLocalKnowledgeImportFiles()).map(mapPickedFile)
    },
    async pickImportFolder() {
      return (await client.pickLocalKnowledgeImportFolder()).map(mapPickedFile)
    },
    async startImport(request: LocalKnowledgeImportRequest): Promise<OperationAccepted> {
      const files = request.files.map((file) => {
        if (!file.sourcePath) {
          unavailable('local_knowledge.import_source_path_missing', `无法读取「${file.name}」的本地路径，请使用系统文件选择器重新选择。`)
        }
        return { sourcePath: file.sourcePath, relativePath: file.relativePath }
      })
      return mapAccepted(await client.startLocalKnowledgeImport({
        knowledgeBaseId: request.knowledgeBaseId,
        files,
        parserVersion: request.parserVersion,
        chunkConfigHash: request.chunkConfigHash,
      }))
    },
    async listJobs(knowledgeBaseId) {
      const jobs = await client.listLocalKnowledgeJobs(knowledgeBaseId)
      return jobs.map(mapLocalKnowledgeJob)
    },
    async getOperation(operationId): Promise<OperationSnapshot> {
      const jobs = (await client.listLocalKnowledgeJobs())
        .map(mapLocalKnowledgeJob)
        .filter((job) => job.operationId === operationId || job.id === operationId)
      const event = operationEvent(operationId, jobs)
      return { operationId, lastSequence: event.sequence, lastEvent: event }
    },
    subscribeOperation(operationId, listener) {
      let disposed = false
      let inFlight = false
      let lastSequence = -1
      const poll = async () => {
        if (disposed || inFlight) return
        inFlight = true
        try {
          const snapshot = await this.getOperation(operationId)
          if (!disposed && snapshot.lastEvent && snapshot.lastSequence > lastSequence) {
            lastSequence = snapshot.lastSequence
            listener(snapshot.lastEvent)
          }
        } catch {
        } finally {
          inFlight = false
        }
      }
      void poll()
      const timer = globalThis.setInterval(() => void poll(), 750)
      return () => {
        disposed = true
        globalThis.clearInterval(timer)
      }
    },
    async cancelJob(jobId) {
      return mapLocalKnowledgeJob(await client.cancelLocalKnowledgeJob(jobId))
    },
    async retryJob(jobId) {
      const job = await client.retryLocalKnowledgeJob(jobId)
      return { operationId: job.parentOperationId ?? job.id, acceptedAt: requiredIsoTimestamp(job.updatedAt) }
    },
    async getStorageStatus() {
      return mapLocalKnowledgeStorageStatus(await client.getLocalKnowledgeStorageStatus())
    },
    async pickStorageDirectory() {
      return mapPickedStorageDirectory(await client.pickLocalKnowledgeStorageDirectory())
    },
    async startStorageMigration(destinationPath) {
      const normalizedPath = destinationPath.trim()
      if (!normalizedPath) unavailable('local_knowledge.migration_destination_missing', '请选择新的本地知识库存储文件夹。')
      return mapAccepted(await client.startLocalKnowledgeStorageMigration(normalizedPath))
    },
    async getStorageMigration(migrationId) {
      return mapLocalKnowledgeStorageMigration(await client.getLocalKnowledgeStorageMigration(migrationId))
    },
    async listEmbeddingModels() {
      return (await client.listLocalEmbeddingModels()).map(mapEmbeddingModel)
    },
    async startEmbeddingModelInstall(modelId) {
      return mapAccepted(await client.startLocalEmbeddingModelInstall(modelId))
    },
    async cancelEmbeddingModelDownload(downloadId) {
      return client.cancelLocalEmbeddingModelDownload(downloadId)
    },
    async retryEmbeddingModelDownload(downloadId) {
      return mapAccepted(await client.retryLocalEmbeddingModelDownload(downloadId))
    },
    async testEmbeddingModel(modelId) {
      return client.testLocalEmbeddingModel(modelId)
    },
    async setDefaultEmbeddingModel(modelId) {
      return client.setDefaultLocalEmbeddingModel(modelId)
    },
    async deleteEmbeddingModel(modelId) {
      return client.deleteLocalEmbeddingModel(modelId)
    },
    async importEmbeddingModelPackage(packagePath) {
      return mapEmbeddingModel(await client.importLocalEmbeddingModelPackage(packagePath))
    },
    async getVectorBackendHealth() {
      return client.getLocalVectorBackendHealth()
    },
    async startIndex(knowledgeBaseId, rebuild = false) {
      return mapAccepted(await client.startLocalKnowledgeIndex(knowledgeBaseId, rebuild))
    },
    async testRetrieval(knowledgeBaseId, query, mode, limit) {
      return await client.testLocalKnowledgeRetrieval(knowledgeBaseId, query, mode, limit) as KnowledgeRetrievalResult
    },
    async listRetrievalCases(knowledgeBaseId) {
      return await client.listLocalKnowledgeRetrievalCases(knowledgeBaseId) as KnowledgeRetrievalCase[]
    },
    async saveRetrievalCase(request) {
      return await client.saveLocalKnowledgeRetrievalCase(request) as KnowledgeRetrievalCase
    },
    async deleteRetrievalCase(id) {
      return client.deleteLocalKnowledgeRetrievalCase(id)
    },
    async exportRetrievalCases(knowledgeBaseId) {
      return await client.exportLocalKnowledgeRetrievalCases(knowledgeBaseId) as { json: unknown; markdown: string }
    },
    async listDocumentChunks(knowledgeBaseId, documentId, options) {
      return await client.listLocalKnowledgeDocumentChunks(knowledgeBaseId, documentId, options?.limit, options?.offset) as KnowledgeChunkPreview[]
    },
    async deleteDocument(knowledgeBaseId, documentId) {
      return client.deleteLocalKnowledgeDocument(knowledgeBaseId, documentId)
    },
    async reparseDocument(knowledgeBaseId, documentId) {
      return mapAccepted(await client.reparseLocalKnowledgeDocument(knowledgeBaseId, documentId))
    },
  }
}

export function createDefaultLocalKnowledgeGateway(): LocalKnowledgeGateway {
  return desktopRuntimeAvailable ? createTauriLocalKnowledgeGateway() : mockLocalKnowledgeGateway
}

export const defaultLocalKnowledgeGateway = createDefaultLocalKnowledgeGateway()
