import type { LocalKnowledgeDocumentDto } from '@/features/conversations/api/desktop-client'
import { binaryValue } from '@/features/knowledge/knowledge-cached-preview-source'
import type { LocalKnowledgeDocumentAsk } from '@/features/workspace/types'

export const MAX_COMPOSER_ATTACHMENT_BYTES = 5 * 1024 * 1024
const DOCUMENT_READ_CHUNK_BYTES = 4 * 1024 * 1024

interface DocumentFileClient {
  listLocalKnowledgeDocuments(knowledgeBaseId: string): Promise<LocalKnowledgeDocumentDto[]>
  readLocalKnowledgeDocumentFileRange(knowledgeBaseId: string, documentId: string, start: number, end: number): Promise<unknown>
}

export async function localKnowledgeDocumentFile(payload: LocalKnowledgeDocumentAsk, client: DocumentFileClient, signal: AbortSignal): Promise<File> {
  signal.throwIfAborted()
  const documents = await client.listLocalKnowledgeDocuments(payload.knowledgeBaseId)
  signal.throwIfAborted()
  const document = documents.find((item) => item.id === payload.documentId && item.knowledgeBaseId === payload.knowledgeBaseId)
  if (!document) throw new Error('这份资料已不存在，请刷新资料库后重试')
  if (!Number.isSafeInteger(document.fileSize) || document.fileSize < 0) throw new Error('资料大小无效，请刷新资料库后重试')
  if (document.fileSize > MAX_COMPOSER_ATTACHMENT_BYTES) throw new Error('单个附件不能超过 5 MB')
  const bytes = new Uint8Array(document.fileSize)
  for (let start = 0; start < document.fileSize; start += DOCUMENT_READ_CHUNK_BYTES) {
    signal.throwIfAborted()
    const end = Math.min(start + DOCUMENT_READ_CHUNK_BYTES, document.fileSize) - 1
    const chunk = binaryValue(await client.readLocalKnowledgeDocumentFileRange(payload.knowledgeBaseId, payload.documentId, start, end))
    signal.throwIfAborted()
    if (chunk.byteLength !== end - start + 1) throw new Error('资料读取不完整，请重试')
    bytes.set(chunk, start)
  }
  const current = (await client.listLocalKnowledgeDocuments(payload.knowledgeBaseId))
    .find((item) => item.id === document.id && item.knowledgeBaseId === document.knowledgeBaseId)
  signal.throwIfAborted()
  if (!current || current.currentRevision !== document.currentRevision || current.fileSize !== document.fileSize) {
    throw new Error('资料已更新，请重新打开后重试')
  }
  return new File([bytes], document.displayName, { type: document.mimeType || 'application/octet-stream', lastModified: document.updatedAt })
}
