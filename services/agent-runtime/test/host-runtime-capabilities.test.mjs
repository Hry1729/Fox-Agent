import test from 'node:test'
import assert from 'node:assert/strict'
import { hostRuntimeCapabilities, hostRuntimeCapabilitiesPrompt, selectToolsForHostRuntime } from '../src/host-runtime-capabilities.mjs'
import { composeFoxPrompt } from '../src/prompt-composer.mjs'

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
  // The real SDK receives the two halves through the planning extension, not
  // the joined prompt. A present joined prompt is not evidence of delivery.
  assert.equal(typeof composed.stablePrefix,'string')
  assert.match(composed.dynamicTail,/Host runtime availability for this frozen manifest\/profile:/u)
  assert.match(composed.dynamicTail,/no verified backend/u)
})
test('run_command is hidden from the model only when the Host reports it unavailable',()=>{
  const tools=[{name:'run_command'},{name:'read_file'}]
  const hidden=selectToolsForHostRuntime(tools,facts)
  assert.deepEqual(hidden.tools.map(t=>t.name),['read_file'])
  assert.deepEqual(hidden.excludedTools,[{name:'run_command',reason:'host_runtime_unavailable',reasonCode:'sandbox_unavailable'}])
  const available={...facts,tools:{...facts.tools,run_command:{availability:'available',actions:{start:'available'}}}}
  assert.deepEqual(selectToolsForHostRuntime(tools,available).tools.map(t=>t.name),['run_command','read_file'])
  const unknown={...facts,tools:{...facts.tools,run_command:{availability:'unknown'}}}
  assert.equal(selectToolsForHostRuntime(tools,unknown).tools.length,2)
  // Legacy payload / stale identity = unknown, never hidden.
  assert.equal(selectToolsForHostRuntime(tools,undefined).tools.length,2)
  assert.equal(selectToolsForHostRuntime(tools,facts,{capabilityManifestHash:'other'}).tools.length,2)
})
