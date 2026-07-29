import type { KnowledgeSourceLocator } from '../workspace/types'

export function normalizeKnowledgeSourceLocator(
  event: Record<string, unknown>,
  fallbackChunkId = '',
): KnowledgeSourceLocator {
  const metadata = event.metadata && typeof event.metadata === 'object'
    ? event.metadata as Record<string, unknown>
    : {}
  const pageValue = event.page ?? metadata.page ?? metadata.page_number ?? metadata.page_num
  const page = typeof pageValue === 'number' ? pageValue : Number(pageValue)
  return {
    chunkId: compact(event.chunkId ?? metadata.chunk_id ?? event.id ?? fallbackChunkId) || undefined,
    page: Number.isSafeInteger(page) && page > 0 ? page : undefined,
    anchor: compact(event.anchor ?? metadata.anchor ?? metadata.section ?? metadata.heading) || undefined,
    excerpt: compact(event.excerpt) || undefined,
  }
}

function compact(value: unknown) {
  if (typeof value === 'string') return value.trim()
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  return ''
}
