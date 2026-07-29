import { describe, expect, test } from 'bun:test'
import type { KnowledgeDocumentSourceMetadata } from '../src/features/conversations/model/types'
import { createKnowledgeRangePreviewSource } from '../src/features/knowledge/knowledge-range-preview-source'
import { createKnowledgeCachedPreviewSource } from '../src/features/knowledge/knowledge-cached-preview-source'

const metadata: KnowledgeDocumentSourceMetadata = {
  filename: 'manual.pdf',
  mediaType: 'application/pdf',
  size: 12 * 1024 * 1024,
  sourceRevision: 'sha256:test',
  versionId: null,
  etag: null,
  lastModified: null,
  acceptsRanges: true,
}

describe('knowledge range preview source', () => {
  test('forwards only the exact requested range and never exposes readAll', async () => {
    const requests: Array<[number, number]> = []
    const source = createKnowledgeRangePreviewSource({
      metadata,
      filename: metadata.filename,
      readRange: async (start, end) => {
        requests.push([start, end])
        return new Uint8Array(end - start + 1)
      },
    })

    expect(source.strategy).toBe('range')
    expect('readAll' in source).toBe(false)
    expect((await source.read(1024, 2047)).byteLength).toBe(1024)
    expect(requests).toEqual([[1024, 2047]])
  })

  test('rejects oversized chunks, invalid ranges and reads after close', async () => {
    const source = createKnowledgeRangePreviewSource({
      metadata,
      filename: metadata.filename,
      readRange: async (start, end) => new Uint8Array(end - start + 1),
    })

    expect(source.read(0, 4 * 1024 * 1024)).rejects.toThrow('4 MB')
    expect(source.read(-1, 1)).rejects.toThrow('范围无效')
    await source.close()
    expect(source.read(0, 0)).rejects.toThrow('已经关闭')
  })
})

describe('knowledge cached preview source', () => {
  test('reads complete files in bounded chunks and releases the lease once', async () => {
    const requests: Array<[number, number]> = []
    let releases = 0
    const source = createKnowledgeCachedPreviewSource({
      metadata: { ...metadata, filename: 'manual.docx', size: 9 * 1024 * 1024 },
      cacheKey: 'cache-1',
      size: 9 * 1024 * 1024,
      readCache: async (start, end) => {
        requests.push([start, end])
        return new Uint8Array(end - start + 1)
      },
      releaseCache: async () => { releases += 1 },
    })

    expect((await source.readAll()).byteLength).toBe(9 * 1024 * 1024)
    expect(requests).toEqual([
      [0, 4 * 1024 * 1024 - 1],
      [4 * 1024 * 1024, 8 * 1024 * 1024 - 1],
      [8 * 1024 * 1024, 9 * 1024 * 1024 - 1],
    ])
    await source.close()
    await source.close()
    expect(releases).toBe(1)
    expect(source.read(0, 0)).rejects.toThrow('已经关闭')
  })

  test('stops between chunks when the viewer is aborted', async () => {
    const controller = new AbortController()
    let reads = 0
    let releases = 0
    const source = createKnowledgeCachedPreviewSource({
      metadata: { ...metadata, filename: 'manual.xlsx' },
      cacheKey: 'cache-2',
      size: metadata.size,
      signal: controller.signal,
      readCache: async (start, end) => {
        reads += 1
        controller.abort()
        return new Uint8Array(end - start + 1)
      },
      releaseCache: async () => { releases += 1 },
    })

    expect(source.readAll()).rejects.toMatchObject({ name: 'AbortError' })
    expect(reads).toBe(1)
    await source.close()
    expect(releases).toBe(1)
  })

  test('rejects truncated cache range responses', async () => {
    const source = createKnowledgeCachedPreviewSource({
      metadata: { ...metadata, size: 2 },
      cacheKey: 'cache-3',
      size: 2,
      readCache: async () => new Uint8Array(1),
      releaseCache: async () => undefined,
    })

    expect(source.read(0, 1)).rejects.toThrow('字节长度')
    await source.close()
  })
})
