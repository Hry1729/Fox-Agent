import { describe, expect, test } from 'bun:test'
import { activityIsStale, compatibleReadingState } from '../src/features/knowledge/document-activity-model'
import type { KnowledgeDocumentBookmark, KnowledgeDocumentReadingState } from '../src/features/conversations/model/types'

const readingState: KnowledgeDocumentReadingState = {
  knowledgeBaseId: 'kb-1',
  documentId: 'doc-1',
  sourceRevision: 'revision-a',
  page: 7,
  scrollOffset: 120,
  zoom: 1.25,
  updatedAt: 1,
}

describe('document activity version compatibility', () => {
  test('restores reading state only for the same source revision', () => {
    expect(compatibleReadingState(readingState, 'revision-a')).toBe(readingState)
    expect(compatibleReadingState(readingState, 'revision-b')).toBeNull()
  })

  test('keeps revision-less compatibility data usable', () => {
    expect(compatibleReadingState({ ...readingState, sourceRevision: null }, 'revision-b')?.page).toBe(7)
  })

  test('marks stale bookmarks without discarding their note data', () => {
    const bookmark: KnowledgeDocumentBookmark = {
      id: 'bookmark-1',
      knowledgeBaseId: 'kb-1',
      documentId: 'doc-1',
      sourceRevision: 'revision-a',
      page: 3,
      anchor: '',
      excerpt: '',
      label: 'Old location',
      createdAt: 1,
      updatedAt: 1,
    }
    expect(activityIsStale(bookmark, 'revision-b')).toBe(true)
    expect(bookmark.label).toBe('Old location')
  })
})
