import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'

export const KNOWLEDGE_PREVIEW_RANGE_CHUNK_SIZE = 4 * 1024 * 1024

export interface KnowledgeRangePreviewSource {
  strategy: 'range'
  metadata: KnowledgeDocumentSourceMetadata
  cacheKey: null
  read(start: number, end: number): Promise<Uint8Array>
  close(): Promise<void>
}

export function createKnowledgeRangePreviewSource({
  metadata,
  filename,
  maxBytes,
  readRange,
}: {
  metadata: KnowledgeDocumentSourceMetadata
  filename: string
  maxBytes?: number
  readRange: (start: number, end: number) => Promise<Uint8Array>
}): KnowledgeRangePreviewSource {
  validateSourceSize(metadata.size)
  if (!metadata.acceptsRanges) throw new Error(`${filename} 的远程服务暂不支持按需读取，请下载原文件查看。`)
  if (maxBytes != null && metadata.size > maxBytes) throw new Error(`${filename} 超过内置预览上限（${formatBytes(maxBytes)}），请下载原文件查看。`)
  let closed = false
  const ensureOpen = () => {
    if (closed) throw new Error('预览文件已经关闭，请重新打开。')
  }
  const read = async (start: number, end: number) => {
    ensureOpen()
    validateReadRange(metadata.size, start, end)
    if (end - start + 1 > KNOWLEDGE_PREVIEW_RANGE_CHUNK_SIZE) throw new Error(`单次预览读取不能超过 ${formatBytes(KNOWLEDGE_PREVIEW_RANGE_CHUNK_SIZE)}。`)
    return readRange(start, end)
  }
  return {
    strategy: 'range',
    metadata,
    cacheKey: null,
    read,
    async close() { closed = true },
  }
}

export function validateReadRange(size: number, start: number, end: number) {
  if (!Number.isSafeInteger(size) || size < 0 || !Number.isSafeInteger(start) || !Number.isSafeInteger(end)
    || start < 0 || end < start || end >= size) {
    throw new Error('预览文件读取范围无效。')
  }
}

export function validateSourceSize(size: number) {
  if (!Number.isSafeInteger(size) || size < 0) throw new Error('知识库文件大小超出当前预览器可安全处理的范围。')
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${Math.round(bytes / 1024 / 1024)} MB`
}
