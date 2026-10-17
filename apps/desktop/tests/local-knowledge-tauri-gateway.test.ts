import { describe, expect, test } from 'bun:test'
import type {
  LocalKnowledgeBaseDto,
  LocalKnowledgeCatalogFileDto,
  LocalKnowledgeDocumentDto,
  LocalKnowledgeFileSourceDto,
  LocalKnowledgeJobDto,
  LocalKnowledgeStorageMigrationDto,
  LocalKnowledgeStorageStatusDto,
} from '../src/features/conversations/api/desktop-client'
import {
  createTauriLocalKnowledgeGateway,
  mapLocalKnowledgeBase,
  type LocalKnowledgeDesktopClient,
} from '../src/features/local-knowledge/tauri-gateway'

const base: LocalKnowledgeBaseDto = {
  id: 'kb-local',
  name: '本机资料',
  description: '测试资料库',
  activeIndexGeneration: 'generation-12',
  documentCount: 2,
  activeJobStatus: null,
  textIndexReady: true,
  vectorIndexReady: false,
  searchMode: 'keyword',
  chunkCount: 8,
  vectorCount: null,
  fallbackReason: '未配置向量模型',
  createdAt: 1_755_800_000_000,
  updatedAt: 1_755_800_060_000,
}

const storage: LocalKnowledgeStorageStatusDto = {
  rootPath: 'D:\\Fox\\knowledge',
  databasePath: 'D:\\Fox\\knowledge\\knowledge.db',
  writable: true,
  schemaVersion: 2,
  pendingRootPath: null,
  restartRequired: false,
  migrationId: null,
}

const migration: LocalKnowledgeStorageMigrationDto = {
  id: 'migration-1',
  sourcePath: 'D:\\Fox\\knowledge',
  destinationPath: 'E:\\Fox\\knowledge',
  status: 'running',
  stage: 'copying',
  progress: 42,
  lastSequence: 3,
  errorCode: null,
  errorMessage: null,
  createdAt: 1_755_800_000_000,
  updatedAt: 1_755_800_050_000,
}

const document: LocalKnowledgeDocumentDto = {
  id: 'document-1',
  knowledgeBaseId: 'kb-local',
  displayName: '产品说明.pdf',
  relativePath: 'docs/产品说明.pdf',
  currentRevision: 3,
  fileSize: 2048,
  mimeType: 'application/pdf',
  parseStatus: 'completed',
  indexStatus: 'indexed',
  chunkCount: 8,
  lastErrorCode: null,
  lastErrorMessage: null,
  createdAt: 1_755_800_000_000,
  updatedAt: 1_755_800_060_000,
}

const fileSource: LocalKnowledgeFileSourceDto = {
  id: 'source-1',
  rootPath: 'D:\\资料',
  displayName: '资料',
  fileCount: 1,
  totalSize: 2048,
  lastScannedAt: 1_755_800_050_000,
  createdAt: 1_755_800_000_000,
  updatedAt: 1_755_800_050_000,
}

const catalogFile: LocalKnowledgeCatalogFileDto = {
  id: 'catalog-1',
  sourceId: 'source-1',
  sourceName: '资料',
  sourcePath: 'D:\\资料',
  absolutePath: 'D:\\资料\\产品说明.pdf',
  relativePath: '产品说明.pdf',
  displayName: '产品说明.pdf',
  extension: 'pdf',
  mimeType: 'application/pdf',
  fileSize: 2048,
  modifiedAt: 1_755_800_050_000,
}

const job: LocalKnowledgeJobDto = {
  id: 'job-1',
  parentOperationId: 'operation-1',
  knowledgeBaseId: 'kb-local',
  jobType: 'index',
  status: 'running',
  stage: 'chunking',
  progress: 42,
  retryCount: 0,
  lastSequence: 5,
  outcome: null,
  generationId: 'generation-12',
  heartbeatAt: 1_755_800_050_000,
  checkpointJson: null,
  errorCode: null,
  errorMessage: null,
  createdAt: 1_755_800_000_000,
  startedAt: 1_755_800_010_000,
  updatedAt: 1_755_800_050_000,
  completedAt: null,
}

function createClient(): LocalKnowledgeDesktopClient {
  return {
    listLocalKnowledgeBases: async () => [base],
    listLocalKnowledgeFileSources: async () => [fileSource],
    pickLocalKnowledgeSourceFolder: async () => 'D:\\资料',
    addLocalKnowledgeFileSource: async (path) => ({ ...fileSource, rootPath: path }),
    rescanLocalKnowledgeFileSource: async () => fileSource,
    removeLocalKnowledgeFileSource: async () => true,
    listLocalKnowledgeLocalFiles: async (options) => {
      expect(options).toMatchObject({ category: 'pdf', offset: 20, limit: 20 })
      return [catalogFile]
    },
    readLocalKnowledgeLocalFileRange: async (id, start, end) => {
      expect({ id, start, end }).toEqual({ id: 'catalog-1', start: 0, end: 3 })
      return [70, 111, 120, 33]
    },
    openLocalKnowledgeLocalFile: async (id, reveal) => {
      expect(id).toBe('catalog-1')
      return reveal === true
    },
    getLocalKnowledgeBase: async () => base,
    createLocalKnowledgeBase: async (request) => {
      expect(request).toEqual({ name: '新建资料', description: '用于测试' })
      return { ...base, id: 'kb-created', name: request.name, description: request.description ?? null }
    },
    listLocalKnowledgeDocuments: async (knowledgeBaseId, query) => {
      expect(knowledgeBaseId).toBe('kb-local')
      expect(query).toBe('产品')
      return [document]
    },
    readLocalKnowledgeDocumentFileRange: async (knowledgeBaseId, documentId, start, end) => {
      expect({ knowledgeBaseId, documentId, start, end }).toEqual({ knowledgeBaseId: 'kb-local', documentId: 'document-1', start: 0, end: 3 })
      return [70, 111, 120, 33]
    },
    openLocalKnowledgeDocumentFile: async (knowledgeBaseId, documentId, reveal) => {
      expect({ knowledgeBaseId, documentId }).toEqual({ knowledgeBaseId: 'kb-local', documentId: 'document-1' })
      return reveal === true
    },
    pickLocalKnowledgeImportFiles: async () => [{
      sourcePath: 'D:\\资料\\产品说明.pdf',
      name: '产品说明.pdf',
      mimeType: 'application/pdf',
      sizeBytes: 2048,
    }],
    startLocalKnowledgeImport: async (request) => {
      expect(request).toEqual({
        knowledgeBaseId: 'kb-local',
        files: [{ sourcePath: 'D:\\资料\\产品说明.pdf' }],
        parserVersion: undefined,
        chunkConfigHash: undefined,
      })
      return { operationId: 'operation-import', acceptedAt: 1_755_800_060_000 }
    },
    listLocalKnowledgeJobs: async () => [job],
    getLocalKnowledgeJob: async () => job,
    cancelLocalKnowledgeJob: async () => ({ ...job, status: 'cancelled', lastSequence: 6 }),
    retryLocalKnowledgeJob: async () => ({ ...job, status: 'queued', lastSequence: 6, parentOperationId: 'operation-retry' }),
    getLocalKnowledgeStorageStatus: async () => storage,
    pickLocalKnowledgeStorageDirectory: async () => ({ path: 'E:\\Fox\\knowledge' }),
    startLocalKnowledgeStorageMigration: async (destinationPath) => {
      expect(destinationPath).toBe('E:\\Fox\\knowledge')
      return { operationId: 'migration-1', acceptedAt: 1_755_800_060_000 }
    },
    getLocalKnowledgeStorageMigration: async (id) => {
      expect(id).toBe('migration-1')
      return migration
    },
  }
}

describe('local knowledge Tauri gateway', () => {
  test('maps Host base, storage and job DTOs to the page model', async () => {
    const gateway = createTauriLocalKnowledgeGateway(createClient())
    const detail = await gateway.getKnowledgeBase('kb-local')

    expect(detail).toMatchObject({
      id: 'kb-local',
      status: 'ready',
      activeGeneration: 12,
      textIndexReady: true,
      vectorIndexReady: false,
      searchMode: 'keyword',
      embeddingModel: null,
      vectorCount: null,
      fallbackReason: '未配置向量模型',
      storagePath: 'D:\\Fox\\knowledge',
      writable: true,
      recentJobs: [{ operationId: 'operation-1', stage: 'chunking', progress: 42 }],
    })
  })

  test('does not infer index readiness from legacy document or generation counts', () => {
    const legacy: LocalKnowledgeBaseDto = { ...base }
    delete legacy.textIndexReady
    delete legacy.vectorIndexReady
    delete legacy.searchMode
    delete legacy.chunkCount
    delete legacy.vectorCount
    delete legacy.fallbackReason

    expect(mapLocalKnowledgeBase(legacy)).toMatchObject({
      status: 'error',
      textIndexReady: false,
      vectorIndexReady: false,
      searchMode: 'keyword',
      chunkCount: null,
      vectorCount: null,
      fallbackReason: 'Host 未提供向量索引状态',
    })
  })

  test('treats an interrupted index job as a terminal error', () => {
    expect(mapLocalKnowledgeBase({ ...base, activeJobStatus: 'interrupted' }).status).toBe('error')
  })

  test('creates a base through the real Host command contract', async () => {
    const gateway = createTauriLocalKnowledgeGateway(createClient())

    await expect(gateway.createKnowledgeBase({ name: '  新建资料  ', description: '  用于测试  ' })).resolves.toMatchObject({
      id: 'kb-created',
      name: '新建资料',
      description: '用于测试',
    })
    await expect(gateway.createKnowledgeBase({ name: '   ' })).rejects.toMatchObject({ code: 'local_knowledge.name_required' })
  })

  test('maps the current camelCase documents contract', async () => {
    const gateway = createTauriLocalKnowledgeGateway(createClient())
    const result = await gateway.listDocuments('kb-local', { query: '产品' })

    expect(result).toEqual({
      total: 1,
      items: [{
        id: 'document-1',
        knowledgeBaseId: 'kb-local',
        name: '产品说明.pdf',
        extension: 'pdf',
        mimeType: 'application/pdf',
        relativePath: 'docs/产品说明.pdf',
        sizeBytes: 2048,
        status: 'indexed',
        chunkCount: 8,
        sourceRevision: '3',
        parseError: null,
        importedAt: '2025-08-21T18:13:20.000Z',
        updatedAt: '2025-08-21T18:14:20.000Z',
      }],
    })
  })

  test('maps selected folders and local file catalog DTOs', async () => {
    const gateway = createTauriLocalKnowledgeGateway(createClient())

    await expect(gateway.listFileSources()).resolves.toEqual([{
      id: 'source-1',
      rootPath: 'D:\\资料',
      displayName: '资料',
      description: '',
      fileCount: 1,
      totalSize: 2048,
      lastScannedAt: '2025-08-21T18:14:10.000Z',
      createdAt: '2025-08-21T18:13:20.000Z',
      updatedAt: '2025-08-21T18:14:10.000Z',
    }])
    await expect(gateway.listLocalFiles({ category: 'pdf', offset: 20, limit: 20 })).resolves.toMatchObject([{
      id: 'catalog-1',
      name: '产品说明.pdf',
      absolutePath: 'D:\\资料\\产品说明.pdf',
      extension: 'pdf',
      sizeBytes: 2048,
    }])
    await expect(gateway.pickFileSourceFolder()).resolves.toBe('D:\\资料')
    await expect(gateway.readLocalFileRange('catalog-1', 0, 3)).resolves.toEqual([70, 111, 120, 33])
    await expect(gateway.openLocalFile('catalog-1')).resolves.toBe(false)
    await expect(gateway.revealLocalFile('catalog-1')).resolves.toBe(true)
  })

  test('uses managed knowledge document preview and system-open commands', async () => {
    const gateway = createTauriLocalKnowledgeGateway(createClient())

    await expect(gateway.readDocumentFileRange('kb-local', 'document-1', 0, 3)).resolves.toEqual([70, 111, 120, 33])
    await expect(gateway.openDocumentFile('kb-local', 'document-1')).resolves.toBe(false)
    await expect(gateway.revealDocumentFile('kb-local', 'document-1')).resolves.toBe(true)
  })

  test('maps cancellation and retry responses', async () => {
    const gateway = createTauriLocalKnowledgeGateway(createClient())
    const cancelled = await gateway.cancelJob('job-1')
    const retried = await gateway.retryJob('job-1')

    expect(cancelled).toMatchObject({ id: 'job-1', status: 'cancelled', lastSequence: 6 })
    expect(retried).toMatchObject({ operationId: 'operation-retry' })
  })

  test('uses Host file selection and import operation data', async () => {
    const gateway = createTauriLocalKnowledgeGateway(createClient())

    const files = await gateway.pickImportFiles?.()
    const accepted = await gateway.startImport({ knowledgeBaseId: 'kb-local', files: files ?? [] })
    const snapshot = await gateway.getOperation('operation-1')

    expect(files).toEqual([{
      sourcePath: 'D:\\资料\\产品说明.pdf',
      name: '产品说明.pdf',
      mimeType: 'application/pdf',
      sizeBytes: 2048,
    }])
    expect(accepted).toEqual({
      operationId: 'operation-import',
      acceptedAt: '2025-08-21T18:14:20.000Z',
    })
    expect(snapshot).toMatchObject({
      operationId: 'operation-1',
      lastSequence: 5,
      lastEvent: {
        operationId: 'operation-1',
        status: 'running',
        stage: 'chunking',
        progress: 42,
        data: { totalItems: 1 },
      },
    })
  })

  test('uses the Host directory picker and maps storage migration state', async () => {
    const gateway = createTauriLocalKnowledgeGateway(createClient())

    await expect(gateway.pickStorageDirectory()).resolves.toBe('E:\\Fox\\knowledge')
    await expect(gateway.startStorageMigration('E:\\Fox\\knowledge')).resolves.toEqual({
      operationId: 'migration-1',
      acceptedAt: '2025-08-21T18:14:20.000Z',
    })
    await expect(gateway.getStorageMigration('migration-1')).resolves.toMatchObject({
      id: 'migration-1',
      status: 'running',
      stage: 'copying',
      progress: 42,
      lastSequence: 3,
    })
    await expect(gateway.getStorageStatus()).resolves.toMatchObject({
      rootPath: 'D:\\Fox\\knowledge',
      pendingRootPath: null,
      restartRequired: false,
    })
  })
})

test('coalesces concurrent reads and invalidates snapshots while jobs are active', async () => {
  const client = createClient()
  let bases = 0, storageReads = 0, details = 0, jobs = 0
  client.listLocalKnowledgeBases = async () => { bases++; return [base] }
  client.getLocalKnowledgeStorageStatus = async () => { storageReads++; return storage }
  client.getLocalKnowledgeBase = async () => { details++; return base }
  client.listLocalKnowledgeJobs = async () => { jobs++; return [job] }
  const gateway = createTauriLocalKnowledgeGateway(client)
  await Promise.all([gateway.listKnowledgeBases(), gateway.listKnowledgeBases(), gateway.getKnowledgeBase(base.id), gateway.getKnowledgeBase(base.id), gateway.listJobs(base.id)])
  expect({ bases, storageReads, details, jobs }).toEqual({ bases: 1, storageReads: 1, details: 1, jobs: 1 })
  await gateway.listKnowledgeBases()
  expect(bases).toBe(2)
  expect(storageReads).toBe(2)
})

test('failed shared reads can be retried', async () => {
  const client = createClient()
  let attempts = 0
  client.listLocalKnowledgeBases = async () => { if (++attempts === 1) throw new Error('temporarily unavailable'); return [base] }
  const gateway = createTauriLocalKnowledgeGateway(client)
  const first = await Promise.allSettled([gateway.listKnowledgeBases(), gateway.listKnowledgeBases()])
  expect(first.every((result) => result.status === 'rejected')).toBe(true)
  expect(attempts).toBe(1)
  expect(await gateway.listKnowledgeBases()).toHaveLength(1)
  expect(attempts).toBe(2)
})

test('reuses unchanged library reads and invalidates after mutations', async () => {
  const client = createClient()
  let reads = 0
  client.listLocalKnowledgeBases = async () => { reads++; return [base] }
  client.listLocalKnowledgeJobs = async () => []
  const gateway = createTauriLocalKnowledgeGateway(client)
  await gateway.listKnowledgeBases()
  await gateway.listKnowledgeBases()
  expect(reads).toBe(1)
  await gateway.createKnowledgeBase({ name: '新建资料', description: '用于测试' })
  await gateway.listKnowledgeBases()
  expect(reads).toBe(2)
})

test('a read started before a mutation cannot refill the snapshot cache', async () => {
  const client = createClient()
  let reads = 0
  let release!: (value: LocalKnowledgeBaseDto[]) => void
  client.listLocalKnowledgeBases = async () => {
    reads++
    if (reads === 1) return new Promise((resolve) => { release = resolve })
    return [{ ...base, name: 'new name' }]
  }
  const gateway = createTauriLocalKnowledgeGateway(client)
  const oldRead = gateway.listKnowledgeBases()
  await gateway.createKnowledgeBase({ name: '新建资料', description: '用于测试' })
  release([base])
  await oldRead
  expect((await gateway.listKnowledgeBases())[0].name).toBe('new name')
  expect(reads).toBe(2)
})

test('terminal job changes invalidate cached library metadata', async () => {
  const client = createClient()
  let reads = 0, version = 1
  client.listLocalKnowledgeBases = async () => { reads++; return [base] }
  client.listLocalKnowledgeJobs = async () => [{ ...job, status: 'completed', lastSequence: version }]
  const gateway = createTauriLocalKnowledgeGateway(client)
  await gateway.listJobs()
  await gateway.listKnowledgeBases()
  version++
  await gateway.listJobs()
  await gateway.listKnowledgeBases()
  expect(reads).toBe(2)
})

test('library cache expires after thirty seconds', async () => {
  const client = createClient()
  let reads = 0
  client.listLocalKnowledgeBases = async () => { reads++; return [base] }
  const realNow = Date.now
  let now = realNow()
  Date.now = () => now
  try {
    const gateway = createTauriLocalKnowledgeGateway(client)
    await gateway.listKnowledgeBases()
    now += 29_000
    await gateway.listKnowledgeBases()
    expect(reads).toBe(1)
    now += 1_001
    await gateway.listKnowledgeBases()
    expect(reads).toBe(2)
  } finally { Date.now = realNow }
})

test('catalog pages are reused and a folder rescan invalidates them', async () => {
  const client = createClient()
  let reads = 0
  client.listLocalKnowledgeLocalFiles = async () => { reads++; return [catalogFile] }
  client.rescanLocalKnowledgeFileSource = async () => fileSource
  const gateway = createTauriLocalKnowledgeGateway(client)
  const page = { sourceId: 'source-1', limit: 2000, offset: 0 }
  await gateway.listLocalFiles(page)
  await gateway.listLocalFiles(page)
  expect(reads).toBe(1)
  await gateway.rescanFileSource('source-1')
  await gateway.listLocalFiles(page)
  expect(reads).toBe(2)
})

test('concurrent folder refreshes share one scan and later visits scan again', async () => {
  const client = createClient()
  let scans = 0
  let release!: () => void
  client.rescanLocalKnowledgeFileSource = async () => { scans++; await new Promise<void>((resolve) => { release = resolve }); return fileSource }
  const gateway = createTauriLocalKnowledgeGateway(client)
  const first = gateway.rescanFileSource('source-1')
  const second = gateway.rescanFileSource('source-1')
  expect(scans).toBe(1)
  release()
  await Promise.all([first, second])
  const third = gateway.rescanFileSource('source-1')
  expect(scans).toBe(2)
  release()
  await third
})
