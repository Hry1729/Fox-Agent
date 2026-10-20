// Evidence scheme 3 (2026-10-20): the model is told which evidence id it can
// cite. `task_evidence_add` lists the Host's legal evidenceType->refKind pairs
// and says what a text-only task cites (a completed tool call, by the
// `details.evidenceRef.refId` its result returns). The list is guidance only;
// the Host stays the authority. It is checked here against the Rust matrix so
// the two cannot drift.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import {
  createHostTools,
  TASK_EVIDENCE_ADD_DESCRIPTION,
  TASK_EVIDENCE_REFERENCE_PAIRS,
  TASK_EVIDENCE_REF_ID_DESCRIPTION,
} from '../src/host-tools.mjs'

const RUST_WORK_GRAPH = fileURLToPath(new URL(
  '../../../apps/desktop/src-tauri/src/database/repositories/work_graph.rs', import.meta.url))

function rustEvidencePairs(source) {
  const block = source.match(/pub const EVIDENCE_REFERENCE_PAIRS[^=]*=\s*&\[([\s\S]*?)\n\];/)
  assert.ok(block, 'EVIDENCE_REFERENCE_PAIRS is defined in work_graph.rs')
  const pairs = {}
  for (const [, evidenceType, kinds] of block[1].matchAll(/\("([a-z_]+)",\s*&\[([^\]]*)\]\)/g)) {
    pairs[evidenceType] = [...kinds.matchAll(/"([a-z_]+)"/g)].map(([, kind]) => kind)
  }
  return pairs
}

test('task_evidence_add lists the Host evidence pairs and the text-only rule', () => {
  const tool = createHostTools(() => {}).find(({ name }) => name === 'task_evidence_add')
  assert.equal(tool.description, TASK_EVIDENCE_ADD_DESCRIPTION)
  for (const [evidenceType, kinds] of Object.entries(TASK_EVIDENCE_REFERENCE_PAIRS)) {
    assert.ok(tool.description.includes(`${evidenceType}->${kinds.join('|')}`), evidenceType)
  }
  assert.match(tool.description, /evidenceRef\.refId/)
  assert.match(tool.description, /text-only task/)
  assert.match(tool.description, /evidenceType=tool_call, refKind=tool_call/)
  assert.match(tool.description, /user message id/)
  // Kept: the existing policy sentence.
  assert.ok(tool.description.startsWith('Attach validated evidence to a task in this conversation. validationCheckType must be allowed'))
  assert.equal(tool.parameters.properties.refId.type, 'string')
  assert.equal(tool.parameters.properties.refId.description, TASK_EVIDENCE_REF_ID_DESCRIPTION)
  // The schema itself is unchanged: same enums, same required keys, closed.
  assert.equal(tool.parameters.additionalProperties, false)
  assert.deepEqual([...tool.parameters.required].sort(),
    ['evidenceType', 'refId', 'refKind', 'summary', 'taskId'])
  assert.deepEqual(tool.parameters.properties.evidenceType.anyOf.map(({ const: value }) => value),
    ['tool_call', 'trace_span', 'test_result', 'file_diff', 'artifact', 'user_confirmation', 'external_reference'])
  assert.deepEqual(tool.parameters.properties.refKind.anyOf.map(({ const: value }) => value),
    ['tool_call', 'artifact', 'run_event', 'message', 'source'])
})

test('the published pairs equal the Host matrix (EVIDENCE_REFERENCE_PAIRS)', { skip: !existsSync(RUST_WORK_GRAPH) && 'Rust source not in this checkout' }, () => {
  const rust = rustEvidencePairs(readFileSync(RUST_WORK_GRAPH, 'utf8'))
  const node = Object.fromEntries(Object.entries(TASK_EVIDENCE_REFERENCE_PAIRS).map(([k, v]) => [k, [...v]]))
  assert.deepEqual(node, rust)
})
