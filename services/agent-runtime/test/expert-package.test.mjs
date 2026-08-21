import test from 'node:test'
import assert from 'node:assert/strict'
import {
  diagnoseToolsForAgentContext,
  filterToolsForAgentContext,
  filterToolsForExpert,
} from '../src/expert-package.mjs'

const tools = [{ name: 'read' }, { name: 'write_file' }, { name: 'run_command' }]

test('expert package enforces an explicit runtime tool allowlist', () => {
  assert.deepEqual(filterToolsForExpert(tools, {
    packageManifest: { allowedTools: ['read', 'unknown'] },
  }).map((tool) => tool.name), ['read'])
})

test('legacy packages keep the catalog while an explicit empty allowlist disables tools', () => {
  assert.equal(filterToolsForExpert(tools, null), tools)
  assert.deepEqual(filterToolsForExpert(tools, { packageManifest: { allowedTools: [] } }), [])
})

test('assistant and expert tool allowlists are intersected', () => {
  const filtered = filterToolsForAgentContext(
    tools,
    { packageManifest: { allowedTools: ['read', 'write_file'] } },
    { packageManifest: { allowedTools: ['read', 'run_command'] } },
  )

  assert.deepEqual(filtered.map((tool) => tool.name), ['read'])
})

test('reports normalized declarations, exclusions, and unknown tools', () => {
  const diagnostics = diagnoseToolsForAgentContext(
    tools,
    { packageManifest: { allowedTools: [' write_file ', 'read', 'unknown', 'read'] } },
    { packageManifest: { allowedTools: ['read', 'run_command', 'unknown'] } },
  )

  assert.deepEqual(diagnostics.assistantDeclaredToolNames, ['read', 'unknown', 'write_file'])
  assert.deepEqual(diagnostics.expertDeclaredToolNames, ['read', 'run_command', 'unknown'])
  assert.deepEqual(diagnostics.effectiveToolNames, ['read'])
  assert.deepEqual(diagnostics.excludedTools, [
    { name: 'write_file', reason: 'expert', reasons: ['expert'] },
    { name: 'run_command', reason: 'assistant', reasons: ['assistant'] },
    { name: 'unknown', reason: 'unregistered', declaredBy: ['assistant', 'expert'] },
  ])
})

test('distinguishes an explicit empty allowlist from an unspecified one', () => {
  const unrestricted = diagnoseToolsForAgentContext(tools, null, null)
  assert.equal(unrestricted.assistantDeclaredToolNames, null)
  assert.equal(unrestricted.expertDeclaredToolNames, null)
  assert.deepEqual(unrestricted.effectiveToolNames, ['read', 'write_file', 'run_command'])

  const disabled = diagnoseToolsForAgentContext(
    tools,
    { packageManifest: { allowedTools: [] } },
    null,
  )
  assert.deepEqual(disabled.assistantDeclaredToolNames, [])
  assert.deepEqual(disabled.effectiveToolNames, [])
  assert.ok(disabled.excludedTools.every((tool) => tool.reason === 'assistant'))
})
