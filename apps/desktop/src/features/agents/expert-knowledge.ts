import type { ExpertPackageManifest, KnowledgeReference } from '@/features/conversations/model/types'

export type ResolvedExpertKnowledgeDeclaration =
  | { format: 'references'; references: KnowledgeReference[] }
  | { format: 'legacy_remote_ids'; ids: string[] }
  | { format: 'empty' }

export class ExpertKnowledgeDeclarationError extends Error {
  readonly code = 'expert.manifest.invalid_knowledge'

  constructor(message: string) {
    super(message)
    this.name = 'ExpertKnowledgeDeclarationError'
  }
}

const hasOwn = (value: object, key: PropertyKey) => Object.prototype.hasOwnProperty.call(value, key)

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}

function requiredString(value: unknown, path: string): string {
  if (typeof value !== 'string' || !value.trim()) {
    throw new ExpertKnowledgeDeclarationError(`${path} 必须是非空字符串`)
  }
  return value
}

function validateReference(value: unknown, index: number): KnowledgeReference {
  const item = record(value)
  if (!item) throw new ExpertKnowledgeDeclarationError(`knowledgeReferences[${index}] 必须是对象`)

  const source = item.source
  const id = requiredString(item.id, `knowledgeReferences[${index}].id`)
  if (item.revision !== undefined) requiredString(item.revision, `knowledgeReferences[${index}].revision`)

  if (source === 'local') {
    return item.revision === undefined ? { source, id } : { source, id, revision: item.revision as string }
  }
  if (source === 'remote') {
    const connectionId = requiredString(item.connectionId, `knowledgeReferences[${index}].connectionId`)
    return item.revision === undefined
      ? { source, connectionId, id }
      : { source, connectionId, id, revision: item.revision as string }
  }
  throw new ExpertKnowledgeDeclarationError(`knowledgeReferences[${index}].source 必须是 local 或 remote`)
}

function validateSchemaVersion(manifest: Record<string, unknown>) {
  if (manifest.manifestSchemaVersion === undefined) return
  if (typeof manifest.manifestSchemaVersion !== 'number' || !Number.isInteger(manifest.manifestSchemaVersion) || manifest.manifestSchemaVersion < 1) {
    throw new ExpertKnowledgeDeclarationError('manifestSchemaVersion 必须是正整数')
  }
}

export function resolveExpertKnowledgeDeclaration(manifest: unknown): ResolvedExpertKnowledgeDeclaration {
  const value = record(manifest)
  if (!value) throw new ExpertKnowledgeDeclarationError('专家 Manifest 必须是对象')
  validateSchemaVersion(value)

  if (hasOwn(value, 'knowledgeReferences')) {
    if (!Array.isArray(value.knowledgeReferences)) {
      throw new ExpertKnowledgeDeclarationError('knowledgeReferences 必须是数组')
    }
    return {
      format: 'references',
      references: value.knowledgeReferences.map(validateReference),
    }
  }

  if (hasOwn(value, 'knowledge')) {
    if (!Array.isArray(value.knowledge) || value.knowledge.some((item) => typeof item !== 'string' || !item.trim())) {
      throw new ExpertKnowledgeDeclarationError('legacy knowledge 必须是非空字符串数组')
    }
    return {
      format: 'legacy_remote_ids',
      ids: value.knowledge,
    }
  }

  return { format: 'empty' }
}

export const DEFAULT_REMOTE_KNOWLEDGE_CONNECTION_ID = 'yuxi-primary'

export function remoteKnowledgeReference(
  id: string,
  connectionId = DEFAULT_REMOTE_KNOWLEDGE_CONNECTION_ID,
): Extract<KnowledgeReference, { source: 'remote' }> {
  return { source: 'remote', connectionId, id }
}

export function knowledgeReferencesFromDeclaration(
  declaration: ResolvedExpertKnowledgeDeclaration,
): KnowledgeReference[] {
  if (declaration.format === 'references') return declaration.references
  if (declaration.format === 'legacy_remote_ids') return declaration.ids.map((id) => remoteKnowledgeReference(id))
  return []
}

export function toPersistedExpertManifest(manifest: ExpertPackageManifest): ExpertPackageManifest {
  const declaration = resolveExpertKnowledgeDeclaration(manifest)
  const { knowledge: _legacyKnowledge, ...withoutLegacyKnowledge } = manifest
  return {
    ...withoutLegacyKnowledge,
    manifestSchemaVersion: 2,
    knowledgeReferences: knowledgeReferencesFromDeclaration(declaration),
  }
}
