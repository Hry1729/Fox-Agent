import test from 'node:test'
import assert from 'node:assert/strict'
import {
  PROTOCOL_NAME,
  PROTOCOL_VERSION,
  createEnvelope,
  validateEnvelope,
} from '../src/protocol.mjs'
import {
  CAPABILITY_MANIFEST_VERSION,
  RUNTIME_TOOL_CATALOG,
  assertRegisteredToolsMatchCatalog,
  createCapabilityManifest,
  validateCapabilityManifest,
} from '../src/runtime-contract.mjs'

test('creates versioned JSONL envelopes', () => {
  const envelope = createEnvelope('request', 'initialize')
  assert.equal(envelope.protocol, PROTOCOL_NAME)
  assert.equal(envelope.version, PROTOCOL_VERSION)
  assert.equal(envelope.kind, 'request')
  assert.equal(envelope.type, 'initialize')
  assert.ok(envelope.id)
})

test('rejects incompatible protocol versions', () => {
  const error = validateEnvelope({
    protocol: PROTOCOL_NAME,
    version: 99,
    kind: 'request',
    id: 'request-1',
    type: 'initialize',
  })
  assert.equal(error, 'unsupported protocol version')
})

test('creates and validates the versioned capability manifest', () => {
  const manifest = createCapabilityManifest()
  assert.equal(manifest.manifestVersion, CAPABILITY_MANIFEST_VERSION)
  assert.equal(manifest.imageInput, false)
  assert.equal(manifest.workLoop, true)
  assert.deepEqual(manifest.tools, RUNTIME_TOOL_CATALOG)
  assert.equal(validateCapabilityManifest(manifest), null)
  assert.deepEqual(
    manifest.tools.find(({ name }) => name === 'graph_readonly_run'),
    { name: 'graph_readonly_run', category: 'project-read', execution: 'runtime', approval: 'none' },
  )
  assert.deepEqual(
    manifest.tools.filter(({ name }) => [
      'graph_readonly_activate',
      'graph_readonly_snapshot_get',
      'graph_readonly_node_start',
      'graph_readonly_node_review',
      'graph_readonly_node_finish',
      'graph_readonly_node_cancel',
      'graph_readonly_accept',
    ].includes(name)),
    [
      { name: 'graph_readonly_activate', category: 'work', execution: 'host', approval: 'none' },
      { name: 'graph_readonly_snapshot_get', category: 'work', execution: 'host', approval: 'none' },
      { name: 'graph_readonly_node_start', category: 'work', execution: 'host', approval: 'none' },
      { name: 'graph_readonly_node_review', category: 'work', execution: 'host', approval: 'none' },
      { name: 'graph_readonly_node_finish', category: 'work', execution: 'host', approval: 'none' },
      { name: 'graph_readonly_node_cancel', category: 'work', execution: 'host', approval: 'none' },
      { name: 'graph_readonly_accept', category: 'work', execution: 'host', approval: 'none' },
    ],
  )
  assert.deepEqual(
    manifest.tools.filter(({ name }) => [
      'task_attempt_start',
      'task_repair_start',
      'task_repair_escalate_start',
      'task_attempt_finish',
    ].includes(name)),
    [
      { name: 'task_attempt_start', category: 'work', execution: 'host', approval: 'none' },
      { name: 'task_repair_start', category: 'work', execution: 'host', approval: 'none' },
      { name: 'task_repair_escalate_start', category: 'work', execution: 'host', approval: 'always' },
      { name: 'task_attempt_finish', category: 'work', execution: 'host', approval: 'none' },
    ],
  )
})

test('accepts a v1 manifest as a normal conversation without work tools', () => {
  const manifest = createCapabilityManifest({
    manifestVersion: 1,
    workLoop: undefined,
    tools: RUNTIME_TOOL_CATALOG.filter(({ category }) => category !== 'work'),
  })
  assert.equal(validateCapabilityManifest(manifest), null)
  assert.equal(
    validateCapabilityManifest({ ...manifest, tools: RUNTIME_TOOL_CATALOG }),
    'work tools require workLoop capability',
  )
})

test('rejects invalid capability manifests and tool registry drift', () => {
  assert.equal(
    validateCapabilityManifest({ ...createCapabilityManifest(), manifestVersion: 99 }),
    'unsupported capability manifest version',
  )
  assert.equal(
    validateCapabilityManifest({
      ...createCapabilityManifest(),
      tools: [RUNTIME_TOOL_CATALOG[0], RUNTIME_TOOL_CATALOG[0]],
    }),
    'tools must not contain duplicates',
  )
  assert.throws(
    () => assertRegisteredToolsMatchCatalog(RUNTIME_TOOL_CATALOG.slice(1)),
    /does not match the capability catalog/,
  )
})
