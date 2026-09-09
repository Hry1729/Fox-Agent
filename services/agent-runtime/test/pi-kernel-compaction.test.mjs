import test from 'node:test'
import assert from 'node:assert/strict'
import { prepareKernelCompaction } from '../src/pi-kernel-compaction.mjs'
import { createEnvelope } from '../src/protocol.mjs'
import { createHash } from 'node:crypto'

const identity = { runId:'r',conversationId:'c',runtimeSessionId:'s',executionProfileId:'legacy' }
function request() {
  const permission={mode:'read_only',projectRoot:null,grants:[]}
  return createEnvelope('request','kernel.compact_context',{...identity,payload:{
    controlBinding:{schemaVersion:1,runId:'r',conversationId:'c',engineId:'pi',executionProfileId:'legacy',
      authority:'authoritative',readOnlyExecutor:'rust',permissionSnapshotId:`sha256:${createHash('sha256').update(JSON.stringify(permission)).digest('hex')}`,
      permission,
      budgets:{approvalWaitMs:1000,modelRequestMs:1000,toolExecutionMs:1000,runExecutionMs:10000}},
    compaction:{schemaVersion:1,runId:'r',turnId:'t',compactionId:'job',inputHash:`sha256:${'a'.repeat(64)}`,
      messages:[{role:'user',content:'old prose'}],maxSummaryBytes:512},
  }})
}

test('compaction request validates identity and never accepts tool history or multimodal input', () => {
  const original=request()
  const before=structuredClone(original)
  assert.equal(prepareKernelCompaction(original,identity).compactionId,'job')
  assert.deepEqual(original,before)
  for (const mutate of [
    value=>{value.payload.compaction.runId='foreign'},
    value=>{value.payload.compaction.messages=[{role:'toolResult',content:[]}]},
    value=>{value.payload.compaction.messages=[{role:'assistant',content:[{type:'toolCall',id:'t',name:'read',arguments:{}}]}]},
    value=>{value.payload.compaction.messages=[{role:'user',content:[{type:'image',data:'x',mimeType:'image/png'}]}]},
    value=>{value.payload.compaction.maxSummaryBytes=8193},
    value=>{value.payload.compaction.inputHash='unbound'},
    value=>{value.payload.controlBinding.authority='legacy'},
  ]) {
    const invalid=request();mutate(invalid)
    assert.throws(()=>prepareKernelCompaction(invalid,identity))
  }
})
