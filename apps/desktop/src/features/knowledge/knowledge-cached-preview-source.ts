import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import { byteRanges } from './knowledge-preview-model'
import { validateReadRange, validateSourceSize } from './knowledge-range-preview-source'

const RANGE_CHUNK_SIZE = 4 * 1024 * 1024

export interface KnowledgeCachedPreviewSource {
  strategy: 'cache'
  metadata: KnowledgeDocumentSourceMetadata
  cacheKey: string
  read(start: number, end: number): Promise<Uint8Array>
  readAll(): Promise<Uint8Array>
  close(): Promise<void>
}

interface KnowledgeCachedPreviewSourceOptions {
  metadata: KnowledgeDocumentSourceMetadata
  cacheKey: string
  size: number
  signal?: AbortSignal
  readCache(start: number, end: number): Promise<unknown>
  releaseCache(): Promise<unknown>
}

export function createKnowledgeCachedPreviewSource({
  metadata,
  cacheKey,
  size,
  signal,
  readCache,
  releaseCache,
}: KnowledgeCachedPreviewSourceOptions): KnowledgeCachedPreviewSource {
  validateSourceSize(size)
  let closed = false

  const ensureReadable = () => {
    if (closed) throw new Error('预览文件已经关闭，请重新打开。')
    if (signal?.aborted) throw previewAbortError()
  }
  const read = async (start: number, end: number) => {
    ensureReadable()
    validateReadRange(size, start, end)
    const bytes = binaryValue(await readCache(start, end))
    ensureReadable()
    if (bytes.byteLength !== end - start + 1) throw new Error('预览缓存返回的字节长度与请求范围不一致。')
    return bytes
  }
  const close = async () => {
    if (closed) return
    closed = true
    await releaseCache().catch(() => false)
  }

  return {
    strategy: 'cache',
    metadata,
    cacheKey,
    read,
    async readAll() {
      ensureReadable()
      const output = new Uint8Array(size)
      for (const { start, end } of byteRanges(size, RANGE_CHUNK_SIZE)) {
        ensureReadable()
        output.set(await read(start, end), start)
      }
      return output
    },
    close,
  }
}

export function previewAbortError() {
  return new DOMException('预览已取消', 'AbortError')
}

export function binaryValue(value: unknown) {
  if (value instanceof ArrayBuffer) return new Uint8Array(value)
  if (ArrayBuffer.isView(value)) {
    const bytes = new Uint8Array(value.byteLength)
    bytes.set(new Uint8Array(value.buffer, value.byteOffset, value.byteLength))
    return bytes
  }
  if (Array.isArray(value)) return Uint8Array.from(value)
  throw new Error('知识库服务返回了无法识别的二进制数据。')
}
