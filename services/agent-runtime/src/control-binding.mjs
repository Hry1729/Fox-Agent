import { createHash } from 'node:crypto'
import { RUNTIME_TOOL_CATALOG, validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'

const knownTools = new Set(RUNTIME_TOOL_CATALOG.map(tool => tool.name))

// The wire schema validates shape; the adapter validates the identity it can
// actually execute. No adapter may silently accept another engine/authority.
export function validatePromptControl(request, executionProfileId) {
  const binding = request.payload?.controlBinding
  // Protocol v1 hosts predating frozen control remain supported. Explicit null
  // or malformed bindings are not the same as an absent compatibility field.
  if (!Object.hasOwn(request.payload ?? {}, 'controlBinding')) return null
  const errors = validateWireValue('RunControlBinding', binding)
  if (errors.length) throw new Error(`Invalid frozen Run control: ${errors.join('; ')}`)
  if (binding.schemaVersion !== 1) throw new Error('Unsupported frozen Run control version')
  if (binding.engineId !== 'pi' || binding.authority !== 'legacy') {
    throw new Error('Pi Legacy adapter cannot execute the frozen Run engine/authority')
  }
  if (!binding.runId.trim() || !binding.conversationId.trim()
      || binding.runId !== request.runId || binding.conversationId !== request.conversationId
      || binding.executionProfileId !== executionProfileId) {
    throw new Error('Frozen Run control does not match the request identity/profile')
  }
  for (const value of Object.values(binding.budgets)) {
    if (!Number.isSafeInteger(value) || value <= 0 || value > 86_400_000) {
      throw new Error('Frozen Run time budgets must be positive and at most 24 hours')
    }
  }
  if (binding.permission.projectRoot != null && !binding.permission.projectRoot.trim()) {
    throw new Error('Frozen Run project root is empty')
  }
  if (binding.permission.grants.some(grant => !knownTools.has(grant.tool) || !grant.scope.trim())) {
    throw new Error('Invalid frozen Run permission grant')
  }
  // Field order matches Rust FrozenPermission / PermissionGrant serialization,
  // independent of property order in the incoming JSON transport.
  const permission = {
    mode: binding.permission.mode,
    projectRoot: binding.permission.projectRoot ?? null,
    grants: binding.permission.grants.map(grant => ({ tool: grant.tool, scope: grant.scope })),
  }
  const hash = `sha256:${createHash('sha256').update(JSON.stringify(permission)).digest('hex')}`
  if (hash !== binding.permissionSnapshotId) throw new Error('Frozen Run permission hash mismatch')
  const project = request.payload?.projectContext
  if (project && (project.projectRoot !== permission.projectRoot || project.permissionMode !== permission.mode)) {
    throw new Error('Prompt project context disagrees with frozen Run permission')
  }
  return { projectRoot: permission.projectRoot, permissionMode: permission.mode }
}
