import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { Check } from 'typebox/value'
import { createHostTools } from '../src/host-tools.mjs'
import { describeKernelRun } from '../src/pi-kernel-description.mjs'
import { diagnoseToolsForAgentContext, selectToolsForProjectContext } from '../src/expert-package.mjs'
import { RUNTIME_TOOL_CATALOG } from '../src/runtime-contract.mjs'

const bundled=JSON.parse(readFileSync(new URL('../../../apps/desktop/src-tauri/resources/expert-library/bundled.json',import.meta.url),'utf8'))
const data=bundled.find(entry=>entry.expertId==='fox-data-analyst').package
const expert={systemPrompt:data.files['./prompts/system.md'].content,packageManifest:{allowedTools:data.resources.tools}}
// Kept equal to the persisted built-in assistant by a Rust database upgrade test.
const defaultAssistant={packageManifest:{allowedTools:JSON.parse(readFileSync(new URL('./fixtures/default-assistant-tools.json',import.meta.url),'utf8'))}}

test('data expert exposes actual code computation without project or arbitrary shell',()=>{
  const catalog=createHostTools(()=>{throw new Error('description must not execute')})
  const effective=selectToolsForProjectContext(diagnoseToolsForAgentContext(catalog,defaultAssistant,expert).tools,{projectRoot:null,permissionMode:'read_only'}).tools
  assert.ok(effective.some(tool=>tool.name==='attachment_compute'))
  assert.ok(!effective.some(tool=>['run_command','write_file','tabular_data'].includes(tool.name)))
  const describe=assistantPackage=>describeKernelRun({conversationId:'compute-test',payload:{executionProfileId:'legacy',modelService:{modelId:'test-model',apiType:'openai-completions'},supportedTools:RUNTIME_TOOL_CATALOG.map(t=>t.name),prompt:{systemPrompt:'Analyze the attached workbook',projectContext:{projectRoot:null,permissionMode:'read_only'},expertPackage:expert,assistantPackage}}})
  const prepared=describe(defaultAssistant)
  assert.ok(prepared.proposalTools.some(t=>t.name==='attachment_compute'))
  assert.match(prepared.systemPrompt,/Never replace tool execution with mental arithmetic/)
  assert.match(prepared.systemPrompt,/isolated conversation workspace are permitted/)
  assert.ok(!describe({packageManifest:{allowedTools:[]}}).proposalTools.some(t=>t.name==='attachment_compute'))
  const beforeFix={packageManifest:{allowedTools:defaultAssistant.packageManifest.allowedTools.filter(name=>name!=='attachment_compute')}}
  assert.ok(!describe(beforeFix).proposalTools.some(t=>t.name==='attachment_compute'))
})

test('compute schema accepts bounded code and forwards attachment IDs intact',async()=>{
  const requests=[]
  const tool=createHostTools(async(type,payload)=>{requests.push({type,payload});return {payload:{result:{details:{result:{count:927}}}}}}).find(t=>t.name==='attachment_compute')
  const input={attachmentIds:['attachment-1'],code:'return {count: attachments[0].sheets[0].rows.length - 1};',timeoutMs:1000}
  assert.ok(Check(tool.parameters,input))
  assert.ok(Check(tool.parameters,{artifactIds:["computed-file"],code:"return attachments[0].data;"}))
  assert.ok(!Check(tool.parameters,{...input,attachmentIds:[]}))
  assert.ok(!Check(tool.parameters,{...input,timeoutMs:30001}))
  assert.ok(!Check(tool.parameters,{...input,code:'x'.repeat(131073)}))
  const result=await tool.execute('call-compute',input,new AbortController().signal)
  assert.equal(result.details.result.count,927)
  assert.deepEqual(requests,[{type:'tool.execute',payload:{toolCallId:'call-compute',tool:'attachment_compute',input}}])
})
