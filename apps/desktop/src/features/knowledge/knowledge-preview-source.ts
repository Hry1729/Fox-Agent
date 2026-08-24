import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import { binaryValue, createKnowledgeCachedPreviewSource, previewAbortError, type KnowledgeCachedPreviewSource } from './knowledge-cached-preview-source'
import { createKnowledgeRangePreviewSource, validateSourceSize, type KnowledgeRangePreviewSource } from './knowledge-range-preview-source'

export { binaryValue, createKnowledgeCachedPreviewSource, type KnowledgeCachedPreviewSource } from './knowledge-cached-preview-source'

export interface KnowledgeCachedPreviewOpenOptions {
  maxBytes: number
  signal?: AbortSignal
}

export type KnowledgeCachedPreviewOpener = (
  options: KnowledgeCachedPreviewOpenOptions,
) => Promise<KnowledgeCachedPreviewSource>

export async function openKnowledgeRangePreviewSource({
  knowledgeBaseId,
  documentId,
  filename,
  maxBytes,
  metadata: suppliedMetadata,
}: {
  knowledgeBaseId: string
  documentId: string
  filename: string
  maxBytes?: number
  metadata?: KnowledgeDocumentSourceMetadata
}): Promise<KnowledgeRangePreviewSource> {
  const metadata = suppliedMetadata ?? await desktopClient.getKnowledgeDocumentSourceMetadata(knowledgeBaseId, documentId)
  return createKnowledgeRangePreviewSource({
    metadata,
    filename,
    maxBytes,
    readRange: (start, end) => desktopClient.readKnowledgeDocumentRange(
      knowledgeBaseId,
      documentId,
      start,
      end,
      metadata.sourceRevision,
    ).then(binaryValue),
  })
}

export async function openKnowledgeCachedPreviewSource({
  knowledgeBaseId,
  documentId,
  filename,
  maxBytes,
  metadata: suppliedMetadata,
  signal,
}: {
  knowledgeBaseId: string
  documentId: string
  filename: string
  maxBytes: number
  metadata?: KnowledgeDocumentSourceMetadata
  signal?: AbortSignal
}): Promise<KnowledgeCachedPreviewSource> {
  const metadata = suppliedMetadata ?? await desktopClient.getKnowledgeDocumentSourceMetadata(knowledgeBaseId, documentId)
  validateSourceSize(metadata.size)
  if (metadata.size > maxBytes) {
    throw new Error(`${filename} 超过内置预览上限（${formatBytes(maxBytes)}），请下载原文件查看。`)
  }
  const operationId = crypto.randomUUID()
  const cancel = () => { void desktopClient.cancelKnowledgePreviewCache(operationId).catch(() => false) }
  if (signal?.aborted) throw new DOMException('预览已取消', 'AbortError')
  let operationRegistered = false
  let lease
  try {
    await desktopClient.registerKnowledgePreviewCacheOperation(operationId)
    operationRegistered = true
    signal?.addEventListener('abort', cancel, { once: true })
    if (signal?.aborted) {
      cancel()
      throw new DOMException('预览已取消', 'AbortError')
    }
    lease = await desktopClient.acquireKnowledgePreviewCache({
      operationId,
      knowledgeBaseId,
      documentId,
      sourceRevision: metadata.sourceRevision,
      mediaType: metadata.mediaType,
      size: metadata.size,
      filename,
    })
  } catch (cause) {
    if (signal?.aborted) throw new DOMException('预览已取消', 'AbortError')
    throw cause
  } finally {
    signal?.removeEventListener('abort', cancel)
    if (operationRegistered) {
      await desktopClient.finishKnowledgePreviewCacheOperation(operationId).catch(() => false)
    }
  }
  const source = createKnowledgeCachedPreviewSource({
    metadata,
    cacheKey: lease.cacheKey,
    size: lease.size,
    signal,
    readCache: (start, end) => desktopClient.readKnowledgePreviewCache(lease.cacheKey, start, end),
    releaseCache: () => desktopClient.releaseKnowledgePreviewCache(lease.cacheKey),
  })
  if (signal?.aborted) {
    await source.close()
    throw previewAbortError()
  }
  return source
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${Math.round(bytes / 1024 / 1024)} MB`
}
