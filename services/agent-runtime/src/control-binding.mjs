import { createHash } from 'node:crypto'
import { RUNTIME_TOOL_CATALOG, validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'

const knownTools = new Set(RUNTIME_TOOL_CATALOG.map(tool => tool.name))

// The wire schema validates shape; the adapter validates the identity it can
// actually execute. No adapter may silently accept another engine/authority.
export function validatePromptControl(request, executionProfileId) {
  return validateAdapterControl(request, executionProfileId, 'legacy', true)
}

// Separate entry point: this does NOT enable authoritative prompts in the
// Legacy sidecar. Only the controlled Kernel handoff may use this validator.
export function validateKernelControl(request, executionProfileId, engineId = 'pi') {
  if (!['pi', 'codex', 'deepseek_harness'].includes(engineId)) throw new Error('Unsupported Kernel adapter')
  return validateAdapterControl(request, executionProfileId, 'authoritative', false, engineId)
}

function validateAdapterControl(request, executionProfileId, authority, allowAbsent, engineId = 'pi') {
  const binding = request.payload?.controlBinding
  // Protocol v1 hosts predating frozen control remain supported. Explicit null
  // or malformed bindings are not the same as an absent compatibility field.
  if (allowAbsent && !Object.hasOwn(request.payload ?? {}, 'controlBinding')) return null
  const errors = validateWireValue('RunControlBinding', binding)
  if (errors.length) throw new Error(`Invalid frozen Run control: ${errors.join('; ')}`)
  if (binding.schemaVersion !== 1) throw new Error('Unsupported frozen Run control version')
  if (binding.engineId !== engineId || binding.authority !== authority) {
    throw new Error(`Pi ${authority === 'legacy' ? 'Legacy' : 'Kernel'} adapter cannot execute the frozen Run engine/authority`)
  }
  if (!binding.runId.trim() || !binding.conversationId.trim()
      || binding.runId !== request.runId || binding.conversationId !== request.conversationId
      || binding.executionProfileId !== executionProfileId) {
    throw new Error('Frozen Run control does not match the request identity/profile')
  }
  for (const [key, value] of Object.entries(binding.budgets)) {
    if (key === 'runExecutionLimited') {
      if (typeof value !== 'boolean') throw new Error('Invalid frozen Run duration policy')
      continue
    }
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
  // Field order and field set match Rust `FrozenPermission` / `PermissionGrant`
  // serialization exactly, independent of property order in the incoming JSON
  // transport. `kind` and `approvalEpoch` are part of the canonical form: a
  // grant without a kind is a revocable approval-reuse grant, and a binding
  // without an epoch predates the field.
  const permission = canonicalPermission(binding.permission)
  const hash = `sha256:${createHash('sha256').update(JSON.stringify(permission)).digest('hex')}`
  if (hash !== binding.permissionSnapshotId) throw new Error('Frozen Run permission hash mismatch')
  const project = request.payload?.projectContext
  if (project && (project.projectRoot !== permission.projectRoot || project.permissionMode !== permission.mode)) {
    throw new Error('Prompt project context disagrees with frozen Run permission')
  }
  return { projectRoot: permission.projectRoot, permissionMode: permission.mode }
}

/// The canonical `FrozenPermission` form, field-for-field in Rust serialization
/// order. Both the validator and the test fixtures hash exactly this, so a
/// fixture can never drift from what the Host actually signs.
export function canonicalPermission(source) {
  return {
    mode: source.mode,
    projectRoot: source.projectRoot ?? null,
    grants: (source.grants ?? []).map(grant => ({
      tool: grant.tool,
      scope: grant.scope,
      kind: grant.kind ?? 'approval_reuse',
    })),
    approvalEpoch: source.approvalEpoch ?? null,
  }
}
