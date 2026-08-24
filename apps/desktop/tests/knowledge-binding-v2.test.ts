import { describe, expect, test } from 'bun:test'
import {
  knowledgeReferenceFromLegacyBinding,
  knowledgeReferenceKey,
  normalizeKnowledgeBindingsSetPayload,
  normalizeKnowledgeReference,
} from '../src/features/conversations/api/desktop-client'

describe('conversation knowledge binding v2 compatibility', () => {
  test('normalizes a legacy remote binding without changing its identity', () => {
    const reference = knowledgeReferenceFromLegacyBinding({
      conversationId: 'conversation-1',
      serviceConnectionId: 'yuxi-primary',
      knowledgeBaseId: 'remote-1',
      knowledgeBaseName: '远程资料',
      enabled: true,
      createdAt: 1,
      updatedAt: 1,
    })

    expect(reference).toEqual({
      source: 'remote',
      providerKey: 'yuxi-primary',
      connectionId: 'yuxi-primary',
      id: 'remote-1',
    })
    expect(normalizeKnowledgeBindingsSetPayload([{
      conversationId: 'conversation-1',
      serviceConnectionId: 'yuxi-primary',
      knowledgeBaseId: 'remote-1',
      knowledgeBaseName: '远程资料',
      enabled: true,
      createdAt: 1,
      updatedAt: 1,
    }]).knowledgeReferences).toEqual([reference])
  })

  test('keeps an explicitly empty v2 reference list empty', () => {
    expect(normalizeKnowledgeBindingsSetPayload({
      bindings: [],
      knowledgeReferences: [],
    }).knowledgeReferences).toEqual([])
  })

  test('normalizes provider keys and keeps local and remote IDs distinct', () => {
    const local = normalizeKnowledgeReference({ source: 'local', id: 'same-id' })
    const remote = normalizeKnowledgeReference({ source: 'remote', connectionId: 'yuxi-primary', id: 'same-id' })

    expect(local).toMatchObject({ source: 'local', providerKey: 'local', id: 'same-id' })
    expect(remote).toMatchObject({ source: 'remote', providerKey: 'yuxi-primary', id: 'same-id' })
    expect(knowledgeReferenceKey(local)).not.toBe(knowledgeReferenceKey(remote))
  })
})
