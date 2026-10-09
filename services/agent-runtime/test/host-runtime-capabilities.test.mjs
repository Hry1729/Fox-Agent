import test from 'node:test'
import assert from 'node:assert/strict'
import { hostRuntimeCapabilities, hostRuntimeCapabilitiesPrompt } from '../src/host-runtime-capabilities.mjs'
import { composeFoxPrompt } from '../src/prompt-composer.mjs'
import { compactionFailureDiagnostic } from '../src/pi-kernel-compaction.mjs'

const facts={schemaVersion:1,capabilityManifestHash:'sha256:manifest',executionProfileId:'legacy',tools:{
  run_command:{availability:'unavailable',reasonCode:'sandbox_unavailable',reason:'no verified backend',actions:{start:'unavailable',status:'available',output:'available',cancel:'available'}},
  attachment_compute:{availability:'available',projectPaths:true,reportPdf:{availability:'unavailable',reasonCode:'tool.pdf_font_unavailable'}},
}}
test('Host availability remains separate from schema and rejects stale manifest/profile identities',()=>{
  assert.equal(hostRuntimeCapabilities(undefined),null)
  assert.equal(hostRuntimeCapabilities(facts,{capabilityManifestHash:'other'}),null)
  assert.equal(hostRuntimeCapabilities(facts,{executionProfileId:'durable_v2'}),null)
  assert.equal(hostRuntimeCapabilities(facts).tools.run_command.actions.status,'available')
  assert.equal(hostRuntimeCapabilities(facts).tools.attachment_compute.reportPdf.availability,'unavailable')
  assert.match(hostRuntimeCapabilitiesPrompt(facts),/Do not call an unavailable action/)
})
test('Host availability survives bounded prompt composition',()=>{
  const composed=composeFoxPrompt({context:{hostRuntimeCapabilities:facts,capabilityManifestHash:facts.capabilityManifestHash,executionProfile:{id:'legacy'}},budget:{maxPromptTokens:8000}})
  assert.match(composed.prompt,/no verified backend/)
  assert.match(composed.prompt,/sandbox_unavailable/)
  assert.match(composed.prompt,/schemas describe contracts/)
})
test('Compaction records safe reasons while preserving unknown outcome retry refusal',()=>{
  const request={runId:'r',payload:{compaction:{compactionId:'c'}}}
  const diagnostic=compactionFailureDiagnostic(new Error('fetch failed token=secret https://host/private?api_key=abc sk-secret123456'),request)
  assert.equal(diagnostic.category,'unknown')
  assert.equal(diagnostic.outcomeKnown,false)
  assert.equal(diagnostic.retryable,false)
  assert.doesNotMatch(diagnostic.upstreamMessage,/secret123456|token=secret|private\?api_key/)
  const provider=compactionFailureDiagnostic(new Error('provider unavailable'),request,{rejection:{category:'provider_unavailable',httpStatus:503},final:{role:'assistant',stopReason:'error',content:[]}})
  assert.equal(provider.category,'provider_unavailable')
  assert.equal(provider.retryable,true)
  assert.equal(provider.httpStatus,503)
  const partial=compactionFailureDiagnostic(new Error('stream failed'),request,{rejection:{category:'provider_unavailable',httpStatus:503},final:{role:'assistant',stopReason:'error',content:[{type:'text',text:'partial'}]}})
  assert.equal(partial.retryable,false)
})
