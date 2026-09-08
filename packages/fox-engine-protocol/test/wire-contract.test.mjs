import test from 'node:test'
import assert from 'node:assert/strict'
import { SCHEMA_BUNDLE, PROTOCOL_NAME, PROTOCOL_VERSION, RUNTIME_TOOL_CATALOG, validateWireValue } from '../index.mjs'
import { createCapabilityManifest, validateCapabilityManifest } from '../../../services/agent-runtime/src/runtime-contract.mjs'
import { createEnvelope } from '../../../services/agent-runtime/src/protocol.mjs'

test('initial input snapshot schema preserves identity and rejects extra control fields', () => {
  const input = { schemaVersion: 1, runId: 'r', turnId: 't', promptConfigHash: 'hash', messages: [{ role: 'user', content: '中文 😀' }] }
  assert.deepEqual(validateWireValue('KernelInitialModelInput', JSON.parse(JSON.stringify(input))), [])
  assert.ok(validateWireValue('KernelInitialModelInput', { ...input, apiKey: 'not-a-snapshot-field' }).length)
  assert.ok(validateWireValue('KernelInitialModelInput', { ...input, messages: 'not-an-array' }).length)
  const { turnId, ...missingTurn } = input
  assert.ok(validateWireValue('KernelInitialModelInput', missingTurn).length)
  // History completeness and current-user semantics are checked in Rust;
  // schema validation alone never authorizes an engine dispatch.
})

test('the actual Node request serializes to the Rust-generated wire schema', () => {
  const request = createEnvelope('request', 'run.start', { runId: 'r', payload: { text: 'hello' } })
  assert.equal(request.protocol, PROTOCOL_NAME)
  assert.equal(request.version, PROTOCOL_VERSION)
  assert.deepEqual(validateWireValue('RuntimeRequest', request), [])
  assert.deepEqual(validateWireValue('RuntimeEnvelope', request), [])
  assert.ok(validateWireValue('RuntimeEnvelope', { ...request, version: -1 }).length)
  assert.ok(validateWireValue('RuntimeEnvelope', { ...request, runId: 7 }).length)
  assert.ok(validateWireValue('RuntimeEnvelope', { ...request, seq: Number.MAX_SAFE_INTEGER + 1 }).length)
})

test('the real Node capability manifest uses exactly the Rust canonical catalog', () => {
  const manifest = createCapabilityManifest()
  assert.deepEqual(manifest.tools, SCHEMA_BUNDLE.toolContracts)
  assert.deepEqual(validateWireValue('RuntimeCapabilityManifest', manifest), [])
  assert.equal(validateCapabilityManifest(manifest), null)
  const spoofed = { ...manifest, tools: [{ ...RUNTIME_TOOL_CATALOG.find(tool => tool.name === 'run_command'), approval: 'none' }] }
  assert.match(validateCapabilityManifest(spoofed), /contract mismatch/)
  assert.match(validateCapabilityManifest({ ...manifest, tools: [{ name: 'unknown', category: 'skill', execution: 'host', approval: 'none' }] }), /canonical catalog/)
})

test('version-one optional workLoop remains compatible after JSON serialization', () => {
  const v1 = createCapabilityManifest({ manifestVersion: 1, workLoop: undefined, tools: RUNTIME_TOOL_CATALOG.filter(tool => tool.category !== 'work') })
  assert.equal(validateCapabilityManifest(v1), null)
  assert.equal(validateCapabilityManifest(JSON.parse(JSON.stringify(v1))), null)
  assert.ok(validateWireValue('RuntimeCapabilityManifest', { ...v1, cancellation: undefined }).length)
  assert.ok(validateWireValue('unknown', v1).length)
})
