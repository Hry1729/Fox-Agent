import { describe, expect, test } from 'bun:test'
import {
  resolveExpertKnowledgeDeclaration,
  toPersistedExpertManifest,
} from '../src/features/agents/expert-knowledge'

describe('expert manifest knowledge compatibility', () => {
  test('resolves legacy knowledge IDs as remote declarations', () => {
    expect(resolveExpertKnowledgeDeclaration({ knowledge: ['remote-kb-1', 'remote-kb-2'] })).toEqual({
      format: 'legacy_remote_ids',
      ids: ['remote-kb-1', 'remote-kb-2'],
    })
  })

  test('resolves local and remote knowledge references', () => {
    expect(resolveExpertKnowledgeDeclaration({
      manifestSchemaVersion: 2,
      knowledgeReferences: [
        { source: 'local', id: 'local-kb', revision: 'generation:3' },
        { source: 'remote', connectionId: 'yuxi-primary', id: 'remote-kb' },
      ],
    })).toEqual({
      format: 'references',
      references: [
        { source: 'local', id: 'local-kb', revision: 'generation:3' },
        { source: 'remote', connectionId: 'yuxi-primary', id: 'remote-kb' },
      ],
    })
  })

  test('returns empty only when neither declaration field exists', () => {
    expect(resolveExpertKnowledgeDeclaration({ manifestSchemaVersion: 2 })).toEqual({ format: 'empty' })
  })

  test('rejects malformed references and legacy IDs', () => {
    expect(() => resolveExpertKnowledgeDeclaration({ knowledgeReferences: [{ source: 'remote', id: 'kb' }] })).toThrow()
    expect(() => resolveExpertKnowledgeDeclaration({ knowledgeReferences: [{ source: 'local', id: '' }] })).toThrow()
    expect(() => resolveExpertKnowledgeDeclaration({ knowledge: ['valid-kb', 42] })).toThrow()
  })

  test('gives knowledgeReferences precedence when both fields exist', () => {
    expect(resolveExpertKnowledgeDeclaration({
      knowledgeReferences: [],
      knowledge: [42],
    })).toEqual({ format: 'references', references: [] })
  })

  test('writes new manifests with schema version and without legacy knowledge', () => {
    const saved = toPersistedExpertManifest({ version: '1.3.0', knowledge: ['remote-kb'] })
    expect(saved).toEqual({
      version: '1.3.0',
      manifestSchemaVersion: 2,
      knowledgeReferences: [{ source: 'remote', connectionId: 'yuxi-primary', id: 'remote-kb' }],
    })
    expect('knowledge' in saved).toBe(false)
  })
})
