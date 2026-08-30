import type {
  KnowledgeJob,
  LocalKnowledgeBase,
  LocalKnowledgeBaseDetail,
  LocalKnowledgeBaseCreateRequest,
  LocalKnowledgeCatalogFile,
  LocalKnowledgeDocument,
  LocalKnowledgeFileSource,
  LocalKnowledgeFolder,
  LocalKnowledgeGateway,
  LocalKnowledgeImportRequest,
  LocalKnowledgeStorageMigration,
  LocalKnowledgeStorageStatus,
  OperationAccepted,
  OperationEvent,
  OperationSnapshot,
  Page,
} from './model'

type OperationListener = (event: OperationEvent) => void

function keywordCapability(chunkCount: number | null, lastIndexedAt: string | null, textIndexReady: boolean) {
  return {
    textIndexReady,
    vectorIndexReady: false,
    searchMode: 'keyword' as const,
    configuredEmbeddingModelId: null,
    chunkSize: 512,
    chunkOverlap: 50,
    embeddingModel: null,
    chunkCount,
    vectorCount: null,
    lastIndexedAt,
    fallbackReason: '未配置向量模型',
  }
}

const baseSeed: LocalKnowledgeBase[] = [
  {
    id: 'fox-user-guide',
    name: 'Fox 使用指南',
    description: 'Fox Agent 页面、功能和常用操作的内置使用教程。',
    status: 'ready',
    documentCount: 11,
    activeGeneration: null,
    ...keywordCapability(38, '2026-08-22T09:30:00.000Z', true),
    storagePath: 'FoxData/knowledge/fox-user-guide',
    writable: true,
    lastIndexedAt: '2026-08-22T09:30:00.000Z',
    updatedAt: '2026-08-22T09:30:00.000Z',
  },
  {
    id: 'local-product-docs',
    name: '产品资料库',
    description: '产品手册、发布说明和内部使用指南。',
    status: 'ready',
    documentCount: 18,
    activeGeneration: null,
    ...keywordCapability(426, '2026-08-22T08:20:00.000Z', true),
    storagePath: 'FoxData/knowledge/product-docs',
    writable: true,
    lastIndexedAt: '2026-08-22T08:20:00.000Z',
    updatedAt: '2026-08-22T08:20:00.000Z',
  },
  {
    id: 'local-research-notes',
    name: '研究资料',
    description: '调研记录和待整理的参考资料。',
    status: 'indexing',
    documentCount: 7,
    activeGeneration: null,
    ...keywordCapability(143, null, false),
    storagePath: 'FoxData/knowledge/research-notes',
    writable: true,
    lastIndexedAt: null,
    updatedAt: '2026-08-22T09:03:00.000Z',
  },
]

const guideDocumentSeed: LocalKnowledgeDocument[] = [
  ['guide-intro', '认识 Fox.md', '00-开始使用/01-认识Fox.md'],
  ['guide-first-chat', '第一次对话.md', '00-开始使用/02-第一次对话.md'],
  ['guide-agents', '助手和专家.md', '01-助手与专家/01-助手和专家.md'],
  ['guide-prompts', '如何写清楚需求.md', '01-助手与专家/02-如何写清楚需求.md'],
  ['guide-local-files', '本地文件.md', '02-本地知识/01-本地文件.md'],
  ['guide-local-kb', '本地知识库.md', '02-本地知识/02-本地知识库.md'],
  ['guide-jobs', '任务中心.md', '02-本地知识/03-任务中心.md'],
  ['guide-plugins', '插件中心.md', '03-插件中心/01-插件中心.md'],
  ['guide-capabilities', 'MCP、Skills 与工具.md', '03-插件中心/02-MCP-Skills-工具.md'],
  ['guide-workspace', '工作区和文档.md', '04-工作区与设置/01-工作区和文档.md'],
  ['guide-faq', '常见问题.md', '05-常见问题/01-常见问题.md'],
].map(([id, name, relativePath], index) => ({
  id,
  knowledgeBaseId: 'fox-user-guide',
  name,
  extension: 'md',
  mimeType: 'text/markdown',
  relativePath,
  sizeBytes: 24_000 + index * 512,
  status: 'indexed',
  chunkCount: 3 + (index % 2),
  sourceRevision: `sha256:${id}`,
  parseError: null,
  importedAt: '2026-08-22T09:30:00.000Z',
  updatedAt: '2026-08-22T09:30:00.000Z',
}))

const documentSeed: LocalKnowledgeDocument[] = [
  ...guideDocumentSeed,
  {
    id: 'local-product-readme',
    knowledgeBaseId: 'local-product-docs',
    name: 'Fox 产品手册.pdf',
    extension: 'pdf',
    mimeType: 'application/pdf',
    relativePath: 'manual/Fox 产品手册.pdf',
    sizeBytes: 4_218_880,
    status: 'indexed',
    chunkCount: 132,
    sourceRevision: 'sha256:product-readme',
    parseError: null,
    importedAt: '2026-08-20T09:00:00.000Z',
    updatedAt: '2026-08-22T08:20:00.000Z',
  },
  {
    id: 'local-product-release',
    knowledgeBaseId: 'local-product-docs',
    name: '版本发布说明.md',
    extension: 'md',
    mimeType: 'text/markdown',
    relativePath: 'release/版本发布说明.md',
    sizeBytes: 48_128,
    status: 'indexed',
    chunkCount: 27,
    sourceRevision: 'sha256:product-release',
    parseError: null,
    importedAt: '2026-08-21T10:00:00.000Z',
    updatedAt: '2026-08-21T10:00:00.000Z',
  },
  {
    id: 'local-research-brief',
    knowledgeBaseId: 'local-research-notes',
    name: '用户访谈摘要.docx',
    extension: 'docx',
    mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    relativePath: 'interviews/用户访谈摘要.docx',
    sizeBytes: 1_532_928,
    status: 'parsing',
    chunkCount: 0,
    sourceRevision: null,
    parseError: null,
    importedAt: '2026-08-22T08:48:00.000Z',
    updatedAt: '2026-08-22T09:03:00.000Z',
  },
]

const fileSourceSeed: LocalKnowledgeFileSource[] = [
  {
    id: 'local-file-source-documents',
    rootPath: 'D:\\Documents\\Fox Sources',
    displayName: 'Fox Sources',
    fileCount: 3,
    totalSize: 5_799_936,
    lastScannedAt: '2026-08-22T09:10:00.000Z',
    createdAt: '2026-08-22T09:00:00.000Z',
    updatedAt: '2026-08-22T09:10:00.000Z',
  },
]

const catalogFileSeed: LocalKnowledgeCatalogFile[] = [
  {
    id: 'catalog-product-manual',
    sourceId: 'local-file-source-documents',
    sourceName: 'Fox Sources',
    sourcePath: 'D:\\Documents\\Fox Sources',
    absolutePath: 'D:\\Documents\\Fox Sources\\manual\\Fox 产品手册.pdf',
    relativePath: 'manual\\Fox 产品手册.pdf',
    name: 'Fox 产品手册.pdf',
    extension: 'pdf',
    mimeType: 'application/pdf',
    sizeBytes: 4_218_880,
    modifiedAt: '2026-08-22T08:20:00.000Z',
  },
  {
    id: 'catalog-release-notes',
    sourceId: 'local-file-source-documents',
    sourceName: 'Fox Sources',
    sourcePath: 'D:\\Documents\\Fox Sources',
    absolutePath: 'D:\\Documents\\Fox Sources\\release\\版本发布说明.md',
    relativePath: 'release\\版本发布说明.md',
    name: '版本发布说明.md',
    extension: 'md',
    mimeType: 'text/markdown',
    sizeBytes: 48_128,
    modifiedAt: '2026-08-21T10:00:00.000Z',
  },
  {
    id: 'catalog-interview-brief',
    sourceId: 'local-file-source-documents',
    sourceName: 'Fox Sources',
    sourcePath: 'D:\\Documents\\Fox Sources',
    absolutePath: 'D:\\Documents\\Fox Sources\\interviews\\用户访谈摘要.docx',
    relativePath: 'interviews\\用户访谈摘要.docx',
    name: '用户访谈摘要.docx',
    extension: 'docx',
    mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
    sizeBytes: 1_532_928,
    modifiedAt: '2026-08-22T08:48:00.000Z',
  },
]

const jobSeed: KnowledgeJob[] = [
  {
    id: 'job-research-index',
    operationId: 'operation-research-index',
    knowledgeBaseId: 'local-research-notes',
    type: 'index',
    status: 'running',
    stage: 'chunking',
    progress: 68,
    lastSequence: 4,
    outcome: null,
    completedItems: 119,
    failedItems: 0,
    totalItems: 143,
    heartbeatAt: '2026-08-22T09:03:00.000Z',
    error: null,
    createdAt: '2026-08-22T08:48:00.000Z',
    updatedAt: '2026-08-22T09:03:00.000Z',
  },
  {
    id: 'job-product-import',
    operationId: 'operation-product-import',
    knowledgeBaseId: 'local-product-docs',
    type: 'import',
    status: 'completed',
    stage: 'committing',
    progress: 100,
    lastSequence: 7,
    outcome: 'partial',
    completedItems: 3,
    failedItems: 1,
    totalItems: 4,
    heartbeatAt: null,
    error: null,
    createdAt: '2026-08-21T14:00:00.000Z',
    updatedAt: '2026-08-21T14:08:00.000Z',
  },
]

function copy<T>(value: T): T {
  return structuredClone(value)
}

function now(): string {
  return new Date().toISOString()
}

function extensionFor(name: string): string {
  const extension = name.split('.').pop()?.toLowerCase()
  return extension && extension !== name.toLowerCase() ? extension : 'file'
}

function mockRange(content: string, start: number, end: number): number[] {
  const contentBytes = new TextEncoder().encode(content)
  const requestedLength = Math.max(0, end - start + 1)
  const bytes = new Uint8Array(requestedLength)
  bytes.fill(32)
  if (start < contentBytes.length) bytes.set(contentBytes.slice(start, Math.min(end + 1, contentBytes.length)))
  return Array.from(bytes)
}

function createEvent(
  operationId: string,
  sequence: number,
  status: OperationEvent['status'],
  data: OperationEvent['data'],
  stage: OperationEvent['stage'],
  progress: number,
  entityId?: string,
): OperationEvent {
  return {
    schemaVersion: 1,
    operationId,
    sequence,
    domain: 'knowledge',
    entityId,
    operation: 'documents_import',
    status,
    stage,
    progress,
    data,
    occurredAt: now(),
  }
}

export function createMockLocalKnowledgeGateway(): LocalKnowledgeGateway {
  const bases = new Map(baseSeed.map((base) => [base.id, copy(base)]))
  const fileSources = new Map(fileSourceSeed.map((source) => [source.id, copy(source)]))
  const catalogFiles = new Map(catalogFileSeed.map((file) => [file.id, copy(file)]))
  const documents = new Map<string, LocalKnowledgeDocument[]>()
  const folders = new Map<string, Set<string>>()
  for (const base of baseSeed) folders.set(base.id, new Set())
  for (const document of documentSeed) {
    const collection = documents.get(document.knowledgeBaseId) ?? []
    collection.push(copy(document))
    documents.set(document.knowledgeBaseId, collection)
    const parts = document.relativePath.split('/').filter(Boolean)
    const baseFolders = folders.get(document.knowledgeBaseId) ?? new Set<string>()
    for (let index = 1; index < parts.length; index += 1) baseFolders.add(parts.slice(0, index).join('/'))
    folders.set(document.knowledgeBaseId, baseFolders)
  }

  const jobs = new Map(jobSeed.map((job) => [job.id, copy(job)]))
  const events = new Map<string, OperationEvent>()
  const listeners = new Map<string, Set<OperationListener>>()
  const storageMigrations = new Map<string, LocalKnowledgeStorageMigration>()
  const storageMigrationPolls = new Map<string, number>()
  let storage: LocalKnowledgeStorageStatus = {
    rootPath: 'FoxData/knowledge',
    databasePath: 'FoxData/knowledge/knowledge.db',
    writable: true,
    schemaVersion: 2,
    pendingRootPath: null,
    restartRequired: false,
    migrationId: null,
  }
  let operationCounter = 1
  let documentCounter = 1
  let baseCounter = 1
  let storageMigrationCounter = 1
  let fileSourceCounter = 1

  const publish = (event: OperationEvent) => {
    events.set(event.operationId, copy(event))
    for (const listener of listeners.get(event.operationId) ?? []) listener(copy(event))
  }

  const updateJobFromEvent = (job: KnowledgeJob, event: OperationEvent): KnowledgeJob => ({
    ...job,
    status: event.status,
    stage: event.stage ?? job.stage,
    progress: event.progress ?? job.progress,
    lastSequence: event.sequence,
    outcome: event.data?.outcome ?? job.outcome,
    completedItems: event.data?.completedItems ?? job.completedItems,
    failedItems: event.data?.failedItems ?? job.failedItems,
    totalItems: event.data?.totalItems ?? job.totalItems,
    error: event.error ?? job.error,
    heartbeatAt: event.status === 'running' ? event.occurredAt : null,
    updatedAt: event.occurredAt,
  })

  const createJob = (knowledgeBaseId: string, totalItems: number, type: KnowledgeJob['type']): { job: KnowledgeJob; event: OperationEvent } => {
    const operationId = `mock-operation-${operationCounter++}`
    const jobId = `mock-job-${operationCounter++}`
    const occurredAt = now()
    const event = createEvent(operationId, 1, 'running', {
      completedItems: 0,
      failedItems: 0,
      totalItems,
      outcome: undefined,
    }, 'copying', 8, knowledgeBaseId)
    const job: KnowledgeJob = {
      id: jobId,
      operationId,
      knowledgeBaseId,
      type,
      status: 'running',
      stage: 'copying',
      progress: 8,
      lastSequence: 1,
      outcome: null,
      completedItems: 0,
      failedItems: 0,
      totalItems,
      heartbeatAt: occurredAt,
      error: null,
      createdAt: occurredAt,
      updatedAt: occurredAt,
    }
    jobs.set(job.id, job)
    publish(event)
    return { job, event }
  }

  return {
    async listKnowledgeBases() {
      return [...bases.values()].map(copy)
    },

    async listFileSources() {
      return [...fileSources.values()].map(copy)
    },

    async pickFileSourceFolder() {
      return `D:\\Documents\\Fox Source ${fileSourceCounter}`
    },

    async addFileSource(path) {
      const normalizedPath = path.trim()
      if (!normalizedPath) throw new Error('请选择需要管理的本地文件夹。')
      const existing = [...fileSources.values()].find((source) => source.rootPath.toLocaleLowerCase() === normalizedPath.toLocaleLowerCase())
      if (existing) return copy(existing)
      const timestamp = now()
      const source: LocalKnowledgeFileSource = {
        id: `mock-file-source-${fileSourceCounter++}`,
        rootPath: normalizedPath,
        displayName: normalizedPath.split(/[\\/]/).filter(Boolean).pop() ?? normalizedPath,
        fileCount: 0,
        totalSize: 0,
        lastScannedAt: timestamp,
        createdAt: timestamp,
        updatedAt: timestamp,
      }
      fileSources.set(source.id, source)
      return copy(source)
    },

    async rescanFileSource(id) {
      const source = fileSources.get(id)
      if (!source) throw new Error(`本地文件来源不存在：${id}`)
      const next = { ...source, lastScannedAt: now(), updatedAt: now() }
      fileSources.set(id, next)
      return copy(next)
    },

    async removeFileSource(id) {
      const removed = fileSources.delete(id)
      for (const [fileId, file] of catalogFiles) {
        if (file.sourceId === id) catalogFiles.delete(fileId)
      }
      return removed
    },

    async listLocalFiles(options = {}) {
      const query = options.query?.trim().toLocaleLowerCase() ?? ''
      const categories: Record<Exclude<NonNullable<typeof options.category>, 'all'>, string[]> = {
        text: ['txt', 'md'],
        document: ['docx'],
        pdf: ['pdf'],
        presentation: ['pptx'],
        spreadsheet: ['xlsx'],
      }
      return [...catalogFiles.values()]
        .filter((file) => !options.sourceId || file.sourceId === options.sourceId)
        .filter((file) => !query || [file.name, file.relativePath, file.sourceName].some((value) => value.toLocaleLowerCase().includes(query)))
        .filter((file) => !options.category || options.category === 'all' || categories[options.category].includes(file.extension))
        .slice(options.offset ?? 0, (options.offset ?? 0) + (options.limit ?? 500))
        .map(copy)
    },

    async readLocalFileRange(id, start, end) {
      const file = catalogFiles.get(id)
      if (!file) throw new Error(`本地文件不存在：${id}`)
      const content = file.extension === 'md'
        ? '# Fox 本地文件预览\n\n这是开发环境中的 Markdown 预览示例。\n\n- 点击文件行会在 Fox 中打开\n- 右侧按钮可以加入知识库或调用系统打开\n'
        : file.extension === 'txt'
          ? 'Fox 本地文件预览示例。'
          : ''
      return mockRange(content, start, end)
    },

    async openLocalFile(id) {
      if (!catalogFiles.has(id)) throw new Error(`本地文件不存在：${id}`)
      return true
    },

    async revealLocalFile(id) {
      if (!catalogFiles.has(id)) throw new Error(`本地文件不存在：${id}`)
      return true
    },

    async getKnowledgeBase(id) {
      const base = bases.get(id)
      if (!base) throw new Error(`本地知识库不存在：${id}`)
      return copy({
        ...base,
        parserVersion: 'fox-parser-1',
        storage: { freeBytes: null, totalBytes: null },
        recentJobs: [...jobs.values()].filter((job) => job.knowledgeBaseId === id).sort((left, right) => right.updatedAt.localeCompare(left.updatedAt)).slice(0, 5),
      } satisfies LocalKnowledgeBaseDetail)
    },

    async createKnowledgeBase(request: LocalKnowledgeBaseCreateRequest) {
      const name = request.name.trim()
      if (!name) throw new Error('知识库名称不能为空。')
      const createdAt = now()
      const id = `mock-local-base-${baseCounter++}`
      const base: LocalKnowledgeBase = {
        id,
        name,
        description: request.description?.trim() ?? '',
        status: 'empty',
        documentCount: 0,
        activeGeneration: null,
        ...keywordCapability(0, null, false),
        configuredEmbeddingModelId: request.embeddingModelId ?? null,
        chunkSize: request.chunkSize ?? 512,
        chunkOverlap: request.chunkOverlap ?? 50,
        searchMode: request.searchMode ?? 'keyword',
        storagePath: `FoxData/knowledge/${id}`,
        writable: true,
        lastIndexedAt: null,
        updatedAt: createdAt,
      }
      bases.set(id, base)
      documents.set(id, [])
      folders.set(id, new Set())
      return copy(base)
    },

    async updateKnowledgeBase(id, request) {
      const base = bases.get(id)
      if (!base) throw new Error(`本地知识库不存在：${id}`)
      if (id === 'fox-user-guide') throw new Error('Fox 使用指南是内置知识库，不能修改。')
      const name = request.name.trim()
      if (!name) throw new Error('知识库名称不能为空。')
      const updated = {
        ...base,
        name,
        description: request.description?.trim() ?? '',
        configuredEmbeddingModelId: request.embeddingModelId === undefined ? base.configuredEmbeddingModelId : request.embeddingModelId,
        chunkSize: request.chunkSize ?? base.chunkSize,
        chunkOverlap: request.chunkOverlap ?? base.chunkOverlap,
        searchMode: request.searchMode ?? base.searchMode,
        updatedAt: now(),
      }
      bases.set(id, updated)
      return copy(updated)
    },

    async deleteKnowledgeBase(id) {
      if (id === 'fox-user-guide') throw new Error('Fox 使用指南是内置知识库，不能删除。')
      if (!bases.delete(id)) throw new Error(`本地知识库不存在：${id}`)
      documents.delete(id)
      folders.delete(id)
      for (const [jobId, job] of jobs) {
        if (job.knowledgeBaseId === id) jobs.delete(jobId)
      }
      return true
    },

    async listDocuments(id, options = {}) {
      if (!bases.has(id)) throw new Error(`本地知识库不存在：${id}`)
      const query = options.query?.trim().toLocaleLowerCase() ?? ''
      const items = (documents.get(id) ?? []).filter((document) => !query || [document.name, document.relativePath, document.extension].some((value) => value.toLocaleLowerCase().includes(query)))
      return copy({ items, total: items.length } satisfies Page<LocalKnowledgeDocument>)
    },

    async listFolders(id) {
      if (!bases.has(id)) throw new Error(`本地知识库不存在：${id}`)
      const collection = documents.get(id) ?? []
      return [...(folders.get(id) ?? new Set<string>())]
        .sort((left, right) => left.localeCompare(right, 'zh-CN'))
        .map((relativePath): LocalKnowledgeFolder => ({
          knowledgeBaseId: id,
          name: relativePath.split('/').pop() ?? relativePath,
          relativePath,
          documentCount: collection.filter((document) => document.relativePath.startsWith(`${relativePath}/`)).length,
        }))
        .map(copy)
    },

    async createFolder(knowledgeBaseId, name, parentPath = '') {
      if (!bases.has(knowledgeBaseId)) throw new Error(`本地知识库不存在：${knowledgeBaseId}`)
      const normalizedName = name.trim()
      if (!normalizedName || /[<>:"/\\|?*]/.test(normalizedName) || normalizedName === '.' || normalizedName === '..') throw new Error('文件夹名称无效')
      const relativePath = [parentPath.trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, ''), normalizedName].filter(Boolean).join('/')
      const collection = folders.get(knowledgeBaseId) ?? new Set<string>()
      if (collection.has(relativePath)) throw new Error('文件夹已存在')
      collection.add(relativePath)
      folders.set(knowledgeBaseId, collection)
      return { knowledgeBaseId, name: normalizedName, relativePath, documentCount: 0 }
    },

    async readDocumentFileRange(knowledgeBaseId, documentId, start, end) {
      const document = (documents.get(knowledgeBaseId) ?? []).find((item) => item.id === documentId)
      if (!document) throw new Error(`本地知识文档不存在：${documentId}`)
      const content = document.extension === 'md'
        ? `# ${document.name}\n\n这是 Fox 本地知识库中的文档预览。\n\n- 文档来自本地知识库\n- 可以从左侧文件列表快速切换\n`
        : document.extension === 'txt'
          ? `${document.name}\n\nFox 本地知识文档预览。`
          : ''
      return mockRange(content, start, end)
    },

    async openDocumentFile(knowledgeBaseId, documentId) {
      if (!(documents.get(knowledgeBaseId) ?? []).some((item) => item.id === documentId)) throw new Error(`本地知识文档不存在：${documentId}`)
      return true
    },

    async revealDocumentFile(knowledgeBaseId, documentId) {
      if (!(documents.get(knowledgeBaseId) ?? []).some((item) => item.id === documentId)) throw new Error(`本地知识文档不存在：${documentId}`)
      return true
    },

    async startImport(request: LocalKnowledgeImportRequest): Promise<OperationAccepted> {
      const base = bases.get(request.knowledgeBaseId)
      if (!base) throw new Error(`本地知识库不存在：${request.knowledgeBaseId}`)
      if (!base.writable) throw new Error('当前本地知识库不可写')
      if (request.files.length === 0) throw new Error('至少选择一个文件')

      const { job, event } = createJob(request.knowledgeBaseId, request.files.length, 'import')
      const collection = documents.get(request.knowledgeBaseId) ?? []
      for (const file of request.files) {
        collection.push({
          id: `mock-document-${documentCounter++}`,
          knowledgeBaseId: request.knowledgeBaseId,
          name: file.name,
          extension: extensionFor(file.name),
          mimeType: file.mimeType || 'application/octet-stream',
          relativePath: file.relativePath ?? file.name,
          sizeBytes: file.sizeBytes,
          status: 'queued',
          chunkCount: 0,
          sourceRevision: null,
          parseError: null,
          importedAt: event.occurredAt,
          updatedAt: event.occurredAt,
        })
      }
      documents.set(request.knowledgeBaseId, collection)
      const baseFolders = folders.get(request.knowledgeBaseId) ?? new Set<string>()
      for (const file of request.files) {
        const parts = (file.relativePath ?? file.name).split('/').filter(Boolean)
        for (let index = 1; index < parts.length; index += 1) baseFolders.add(parts.slice(0, index).join('/'))
      }
      folders.set(request.knowledgeBaseId, baseFolders)
      bases.set(request.knowledgeBaseId, {
        ...base,
        status: 'indexing',
        documentCount: collection.length,
        updatedAt: event.occurredAt,
      })
      jobs.set(job.id, updateJobFromEvent(job, event))
      return { operationId: job.operationId, acceptedAt: event.occurredAt }
    },

    async listJobs(knowledgeBaseId) {
      return [...jobs.values()]
        .filter((job) => !knowledgeBaseId || job.knowledgeBaseId === knowledgeBaseId)
        .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
        .map(copy)
    },

    async getOperation(operationId): Promise<OperationSnapshot> {
      const event = events.get(operationId) ?? null
      return { operationId, lastSequence: event?.sequence ?? 0, lastEvent: copy(event) }
    },

    subscribeOperation(operationId, listener) {
      const collection = listeners.get(operationId) ?? new Set<OperationListener>()
      collection.add(listener)
      listeners.set(operationId, collection)
      return () => {
        collection.delete(listener)
        if (collection.size === 0) listeners.delete(operationId)
      }
    },

    async cancelJob(jobId) {
      const job = jobs.get(jobId)
      if (!job) throw new Error(`任务不存在：${jobId}`)
      if (job.status === 'completed' || job.status === 'cancelled') return copy(job)
      const event = createEvent(job.operationId, job.lastSequence + 1, 'cancelled', {
        completedItems: job.completedItems,
        failedItems: job.failedItems,
        totalItems: job.totalItems,
        outcome: job.outcome ?? undefined,
      }, job.stage ?? 'verifying', job.progress, job.knowledgeBaseId)
      const next = updateJobFromEvent(job, event)
      jobs.set(job.id, next)
      publish(event)
      return copy(next)
    },

    async retryJob(jobId) {
      const job = jobs.get(jobId)
      if (!job) throw new Error(`任务不存在：${jobId}`)
      const { job: retry, event } = createJob(job.knowledgeBaseId, job.totalItems, job.type)
      jobs.set(retry.id, updateJobFromEvent(retry, event))
      return { operationId: retry.operationId, acceptedAt: event.occurredAt }
    },

    async getStorageStatus() {
      return copy(storage)
    },

    async pickStorageDirectory() {
      return 'FoxData/knowledge-v2'
    },

    async startStorageMigration(destinationPath) {
      const normalizedPath = destinationPath.trim()
      if (!normalizedPath) throw new Error('请选择新的本地知识库存储文件夹。')
      const active = [...storageMigrations.values()].find((migration) => ['queued', 'running', 'paused'].includes(migration.status))
      if (active) throw new Error('本地知识库存储迁移正在进行中。')
      if (storage.restartRequired) throw new Error('请重启 Fox 后再配置新的存储位置。')
      const nowValue = now()
      const id = `mock-storage-migration-${storageMigrationCounter++}`
      const migration: LocalKnowledgeStorageMigration = {
        id,
        sourcePath: storage.rootPath,
        destinationPath: normalizedPath,
        status: 'running',
        stage: 'copying',
        progress: 18,
        lastSequence: 1,
        error: null,
        createdAt: nowValue,
        updatedAt: nowValue,
      }
      storageMigrations.set(id, migration)
      storageMigrationPolls.set(id, 0)
      storage = { ...storage, pendingRootPath: normalizedPath, migrationId: id }
      return { operationId: id, acceptedAt: nowValue }
    },

    async getStorageMigration(migrationId) {
      const migration = storageMigrations.get(migrationId)
      if (!migration) throw new Error(`存储迁移不存在：${migrationId}`)
      if (migration.status === 'running') {
        const polls = (storageMigrationPolls.get(migrationId) ?? 0) + 1
        storageMigrationPolls.set(migrationId, polls)
        if (polls >= 2) {
          const completed = { ...migration, status: 'completed' as const, stage: 'self_checking' as const, progress: 100, lastSequence: migration.lastSequence + 1, updatedAt: now() }
          storageMigrations.set(migrationId, completed)
          storage = { ...storage, pendingRootPath: completed.destinationPath, restartRequired: true, migrationId }
        } else {
          const running = { ...migration, stage: 'verifying' as const, progress: 72, lastSequence: migration.lastSequence + 1, updatedAt: now() }
          storageMigrations.set(migrationId, running)
        }
      }
      return copy(storageMigrations.get(migrationId)!)
    },
    async listEmbeddingModels() {
      return [{
        id: 'bge-small-zh-v1.5', name: 'BGE Small 中文 v1.5', version: '75c43b0',
        languages: ['中文'], dimension: 512, license: 'MIT', sourceUrl: 'https://huggingface.co/BAAI/bge-small-zh-v1.5',
        status: 'not_installed', integrityStatus: 'pending', isDefault: false, recommended: true,
        sizeBytes: 24_452_059, installedAt: null, packagePath: null, loadReady: false,
        lastErrorCode: null, lastErrorMessage: null, files: [], download: null,
      }]
    },
    async startEmbeddingModelInstall() { return { operationId: `mock-model-${Date.now()}`, acceptedAt: now() } },
    async cancelEmbeddingModelDownload() { return true },
    async retryEmbeddingModelDownload() { return { operationId: `mock-model-${Date.now()}`, acceptedAt: now() } },
    async testEmbeddingModel() { return { integrityVerified: true, loadReady: true, dimension: 512, elapsedMs: 42, message: '测试通过' } },
    async setDefaultEmbeddingModel() { return true },
    async deleteEmbeddingModel() { return true },
    async importEmbeddingModelPackage() { throw new Error('浏览器预览不支持导入本地模型包。') },
    async getVectorBackendHealth() { return { backend: 'mock-vector', available: true, readWriteVerified: true, fallbackActive: false, message: '预览环境向量后端正常。' } },
    async startIndex(knowledgeBaseId) { return { operationId: `mock-index-${knowledgeBaseId}-${Date.now()}`, acceptedAt: now() } },
    async testRetrieval(knowledgeBaseId, query, mode) { return { knowledgeBaseId, query, mode, fusionVersion: mode === 'hybrid' ? 'rrf-v1-k60' : null, timings: { keywordMs: 1, vectorMs: mode === 'keyword' ? 0 : 2, totalMs: mode === 'keyword' ? 1 : 3 }, items: [] } },
    async listRetrievalCases() { return [] },
    async saveRetrievalCase(request) { return { id: request.id ?? `mock-case-${Date.now()}`, knowledgeBaseId: request.knowledgeBaseId, question: request.question, expectedDocumentIds: request.expectedDocumentIds ?? [], expectedKeywords: request.expectedKeywords ?? [], createdAt: Date.now(), updatedAt: Date.now() } },
    async deleteRetrievalCase() { return true },
    async exportRetrievalCases(knowledgeBaseId) { return { json: { schemaVersion: 1, knowledgeBaseId, cases: [] }, markdown: '# 本地知识库检索测试集\n' } },
    async listDocumentChunks() { return [] },
    async deleteDocument() { return true },
    async reparseDocument(knowledgeBaseId) { return { operationId: `mock-reparse-${knowledgeBaseId}-${Date.now()}`, acceptedAt: now() } },
  }
}

export const mockLocalKnowledgeGateway = createMockLocalKnowledgeGateway()
