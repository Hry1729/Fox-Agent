import type {
  KnowledgeDocumentAnnotation,
  KnowledgeDocumentBookmark,
  KnowledgeDocumentReadingState,
} from '@/features/conversations/model/types'

export interface DocumentViewPosition {
  page: number
  scrollOffset?: number
  zoom?: number | null
}

export function compatibleReadingState(
  readingState: KnowledgeDocumentReadingState | null,
  sourceRevision: string | null,
) {
  if (!readingState) return null
  if (readingState.sourceRevision && sourceRevision && readingState.sourceRevision !== sourceRevision) return null
  return readingState
}

export function activityIsStale(
  item: KnowledgeDocumentBookmark | KnowledgeDocumentAnnotation,
  sourceRevision: string | null,
) {
  return Boolean(item.sourceRevision && sourceRevision && item.sourceRevision !== sourceRevision)
}

export function documentActivityOrder(
  left: KnowledgeDocumentBookmark | KnowledgeDocumentAnnotation,
  right: KnowledgeDocumentBookmark | KnowledgeDocumentAnnotation,
) {
  return left.page - right.page || left.createdAt - right.createdAt
}
