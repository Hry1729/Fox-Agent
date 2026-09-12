// Real public Pi session probe; each invocation is a fresh Node process.
//
// capture:        a real Pi session proposes read-a/read-b and is blocked at the
//                 tool_call extension point, so no executor runs.
// resume/resume-propose: a replacement single-round process seeds the Host's
// durable history and settled results, publishes proposal-only schemas and runs
// exactly one model round; it must never re-execute the original tools.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Type } from 'typebox'
import {
  DefaultResourceLoader, SessionManager, SettingsManager, createFoxAgentSession,
  createFoxModelRuntime, registerFauxProvider, fauxAssistantMessage,
} from '../../src/pi-adapter.mjs'
import { installKernelProposalSchemas } from '../../src/pi-kernel-loop.mjs'
import { prepareKernelBatchResume, runPiKernelModel } from '../../src/pi-kernel-batch-resume.mjs'

let input = ''
for await (const chunk of process.stdin) input += chunk
const { request, identity, expectedResults = ['durable result a', 'durable result b'] } = JSON.parse(input)
const mode = process.argv[2]
if (!['capture', 'resume', 'resume-propose'].includes(mode)) throw new Error('unknown probe mode')
const directory = await mkdtemp(join(tmpdir(), 'fox-pi-batch-child-'))
const provider = registerFauxProvider({ api: 'faux-batch-child', provider: 'faux-batch-child', models: [{ id: 'batch-child' }], tokensPerSecond: 1000 })
let executions = 0
let toolExecutionStarts = 0
let providerRequests = 0
let consumed = []
let capture
const captured = new Promise(resolve => { capture = resolve })
let session
const modelRuntime = await createFoxModelRuntime({ model: provider.getModel(), apiKey: 'test-key', fauxRegistration: provider })
const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false, maxRetries: 0, provider: { maxRetries: 0 } } })
const resourceLoader = new DefaultResourceLoader({
  cwd: directory, agentDir: directory, settingsManager, noExtensions: true, noSkills: true,
  noPromptTemplates: true, noThemes: true, noContextFiles: true, systemPrompt: 'Follow the deterministic test script.',
  extensionFactories: mode === 'capture' ? [pi => {
    pi.on('tool_call', async (_event, context) => {
      capture(structuredClone(session.state.messages))
      await new Promise(resolve => {
        if (context.signal.aborted) resolve()
        else context.signal.addEventListener('abort', resolve, { once: true })
      })
      return { block: true, reason: 'controlled process stop after checkpoint' }
    })
  }] : [],
})
try {
  await resourceLoader.reload()
  ;({ session } = await createFoxAgentSession({
    cwd: directory, agentDir: directory, model: provider.getModel(), thinkingLevel: 'off',
    tools: mode === 'capture' ? ['read'] : [], customTools: mode === 'capture' ? [{
      name: 'read', label: 'Probe read', description: 'A test-only tool that must never execute in this probe.',
      parameters: Type.Object({ path: Type.String() }), executionMode: 'parallel',
      execute: async () => { executions++; return { content: [{ type: 'text', text: 'unexpected execution' }] } },
    }] : [], resourceLoader, sessionManager: SessionManager.inMemory(directory), settingsManager, modelRuntime,
  }))
  let output
  if (mode === 'capture') {
    provider.setResponses([fauxAssistantMessage(['a', 'b'].map(suffix => ({ type: 'toolCall', id: `read-${suffix}`, name: 'read', arguments: { path: `${suffix}.txt` } })), { stopReason: 'toolUse' })])
    const prompting = session.prompt('read both files', { expandPromptTemplates: false })
    const messages = await captured
    await session.abort()
    await prompting
    output = { messages, executions }
  } else {
    // Proposal-only schemas are published to the model but never executed: the
    // single-round boundary returns the proposal for the Host to approve.
    installKernelProposalSchemas(session, [{
      name: 'read', description: 'Propose a Host-owned file read.', parameters: Type.Object({ path: Type.String() }),
    }])
    session.agent.subscribe(event => { if (event.type === 'tool_execution_start') toolExecutionStarts++ })
    provider.setResponses([context => {
      providerRequests++
      const results = context.messages.filter(message => message.role === 'toolResult')
      consumed = results.map(message => message.toolCallId)
      if (JSON.stringify(results.map(message => message.content[0]?.text)) !== JSON.stringify(expectedResults)) throw new Error('model did not receive exact durable results')
      if (mode === 'resume-propose') {
        if (JSON.stringify(context.tools?.map(tool => tool.name)) !== JSON.stringify(['read'])) throw new Error('proposal schemas were not advertised')
        return fauxAssistantMessage([{ type: 'toolCall', id: 'read-c', name: 'read', arguments: { path: 'c.txt' } }], { stopReason: 'toolUse' })
      }
      return fauxAssistantMessage('durable batch consumed')
    }])
    const prepared = prepareKernelBatchResume(request, identity)
    prepared.requireCompletion = false
    const resumed = await runPiKernelModel(session, request, prepared, new AbortController().signal, undefined, { allowProposals: true })
    output = { executions, toolExecutionStarts, providerRequests, consumed, response: resumed.response, answer: session.state.messages.at(-1)?.content?.find(block => block.type === 'text')?.text }
  }
  process.stdout.write(JSON.stringify(output))
} finally {
  await session?.abort()
  provider.unregister()
  await rm(directory, { recursive: true, force: true })
}
