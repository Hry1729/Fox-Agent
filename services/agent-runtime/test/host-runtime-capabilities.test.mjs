import test from 'node:test'
import assert from 'node:assert/strict'
import { hostRuntimeCapabilities, hostRuntimeCapabilitiesPrompt, selectToolsForHostRuntime } from '../src/host-runtime-capabilities.mjs'
import { describeKernelRun } from '../src/pi-kernel-description.mjs'
import { RUNTIME_TOOL_CATALOG } from '../src/runtime-contract.mjs'
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
test('kernel description leaves run_command out of the proposal tools when unavailable',()=>{
  const describe=hostRuntimeCapabilities=>describeKernelRun({conversationId:'c',payload:{executionProfileId:'legacy',modelService:{modelId:'m',apiType:'openai-completions'},supportedTools:RUNTIME_TOOL_CATALOG.map(t=>t.name),prompt:{systemPrompt:'s',projectContext:{projectRoot:'/tmp/project',permissionMode:'workspace_write'},hostRuntimeCapabilities}}})
  const names=value=>describe(value).proposalTools.map(t=>t.name)
  assert.ok(names(undefined).includes('run_command'),'baseline offers run_command with a project')
  assert.ok(!names(facts).includes('run_command'))
  // Only run_command goes; every other tool stays.
  assert.deepEqual(names(facts),names(undefined).filter(name=>name!=='run_command'))
})
