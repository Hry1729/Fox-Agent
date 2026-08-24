import { describe, expect, test } from 'bun:test'
import { createMockLocalKnowledgeGateway } from '../src/features/local-knowledge/mock-gateway'

describe('local knowledge mock gateway', () => {
  test('lists local bases and documents without invoking Tauri', async () => {
    const gateway = createMockLocalKnowledgeGateway()
    const bases = await gateway.listKnowledgeBases()
    const documents = await gateway.listDocuments('local-product-docs')

    expect(bases.every((base) => base.writable)).toBe(true)
    expect(documents.items.map((document) => document.name)).toContain('Fox 产品手册.pdf')
  })

  test('manages selected file sources without deleting catalog files implicitly', async () => {
    const gateway = createMockLocalKnowledgeGateway()
    const initialSources = await gateway.listFileSources()
    const initialFiles = await gateway.listLocalFiles({ category: 'pdf' })
    const path = await gateway.pickFileSourceFolder()
    const added = await gateway.addFileSource(path!)

    expect(initialSources).toHaveLength(1)
    expect(initialFiles.map((file) => file.name)).toContain('Fox 产品手册.pdf')
    expect(added.rootPath).toBe(path)
    await expect(gateway.rescanFileSource(added.id)).resolves.toMatchObject({ id: added.id })
    await expect(gateway.removeFileSource(added.id)).resolves.toBe(true)
  })

  test('supports local preview reads and system file actions', async () => {
    const gateway = createMockLocalKnowledgeGateway()
    const [file] = await gateway.listLocalFiles({ category: 'pdf' })

    await expect(gateway.readLocalFileRange(file.id, 0, 3)).resolves.toHaveLength(4)
    await expect(gateway.openLocalFile(file.id)).resolves.toBe(true)
    await expect(gateway.revealLocalFile(file.id)).resolves.toBe(true)
  })

  test('supports paged local files and managed knowledge document previews', async () => {
    const gateway = createMockLocalKnowledgeGateway()
    const firstPage = await gateway.listLocalFiles({ limit: 1, offset: 0 })
    const secondPage = await gateway.listLocalFiles({ limit: 1, offset: 1 })
    const guideDocuments = await gateway.listDocuments('fox-user-guide')
    const guide = guideDocuments.items[0]

    expect(firstPage).toHaveLength(1)
    expect(secondPage).toHaveLength(1)
    expect(secondPage[0].id).not.toBe(firstPage[0].id)
    expect(guideDocuments.total).toBe(11)
    await expect(gateway.readDocumentFileRange('fox-user-guide', guide.id, 0, 3)).resolves.toHaveLength(4)
    await expect(gateway.openDocumentFile('fox-user-guide', guide.id)).resolves.toBe(true)
    await expect(gateway.revealDocumentFile('fox-user-guide', guide.id)).resolves.toBe(true)
  })

  test('creates a local base and makes it available for detail navigation', async () => {
    const gateway = createMockLocalKnowledgeGateway()
    const created = await gateway.createKnowledgeBase({ name: '  新资料库  ', description: '  新的资料  ' })

    expect(created).toMatchObject({
      name: '新资料库',
      description: '新的资料',
      status: 'empty',
      documentCount: 0,
      writable: true,
    })
    await expect(gateway.getKnowledgeBase(created.id)).resolves.toMatchObject({ id: created.id, name: '新资料库' })
    await expect(gateway.createKnowledgeBase({ name: '   ' })).rejects.toThrow('知识库名称不能为空')
  })

  test('accepts an import and exposes a recoverable job snapshot', async () => {
    const gateway = createMockLocalKnowledgeGateway()
    const accepted = await gateway.startImport({
      knowledgeBaseId: 'local-product-docs',
      files: [{ name: '新资料.md', mimeType: 'text/markdown', sizeBytes: 1024 }],
    })
    const jobs = await gateway.listJobs('local-product-docs')
    const job = jobs.find((item) => item.operationId === accepted.operationId)
    const snapshot = await gateway.getOperation(accepted.operationId)

    expect(job).toMatchObject({ status: 'running', totalItems: 1, lastSequence: 1 })
    expect(snapshot).toMatchObject({ operationId: accepted.operationId, lastSequence: 1 })
    expect(snapshot.lastEvent?.data).toMatchObject({ totalItems: 1 })
  })

  test('delivers operation events and supports cancellation', async () => {
    const gateway = createMockLocalKnowledgeGateway()
    const events: number[] = []
    const accepted = await gateway.startImport({
      knowledgeBaseId: 'local-research-notes',
      files: [{ name: '补充资料.txt', mimeType: 'text/plain', sizeBytes: 12 }],
    })
    const jobs = await gateway.listJobs('local-research-notes')
    const job = jobs.find((item) => item.operationId === accepted.operationId)!
    const stop = gateway.subscribeOperation(accepted.operationId, (operationEvent) => events.push(operationEvent.sequence))
    const cancelled = await gateway.cancelJob(job.id)
    stop()

    expect(events).toEqual([2])
    expect(cancelled.status).toBe('cancelled')
    expect(cancelled.lastSequence).toBe(2)
  })

  test('simulates storage directory selection and a restart-required migration', async () => {
    const gateway = createMockLocalKnowledgeGateway()

    await expect(gateway.pickStorageDirectory()).resolves.toBe('FoxData/knowledge-v2')
    const accepted = await gateway.startStorageMigration('FoxData/knowledge-v2')
    await expect(gateway.getStorageStatus()).resolves.toMatchObject({
      pendingRootPath: 'FoxData/knowledge-v2',
      restartRequired: false,
      migrationId: accepted.operationId,
    })

    await gateway.getStorageMigration(accepted.operationId)
    const completed = await gateway.getStorageMigration(accepted.operationId)

    expect(completed).toMatchObject({
      id: accepted.operationId,
      status: 'completed',
      progress: 100,
    })
    await expect(gateway.getStorageStatus()).resolves.toMatchObject({
      pendingRootPath: 'FoxData/knowledge-v2',
      restartRequired: true,
    })
  })
})
