// Contract for the live Kernel engine-side tools installed in the public Pi
// loop. The Host remains the sole concurrency authority: the engine only
// advertises parallel execution for the exact Host-native read-only whitelist
// that the Rust live scheduler fans out. Writers, unknown tools and generic
// MCP tools stay sequential; executors only return Host-settled results.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { installKernelHostTools } from '../src/pi-kernel-loop.mjs'

function idleSession() {
  return {
    isIdle: true,
    agent: {
      state: { tools: [] },
      subscribe() {},
      abort() {},
    },
  }
}

function definition(name) {
  return {
    name,
    description: `${name} tool under test`,
    parameters: { type: 'object', properties: {}, additionalProperties: true },
  }
}

test('only the Host-classified native reader whitelist is advertised parallel', () => {
  const session = idleSession()
  const parallelReaders = ['read', 'ls', 'find', 'grep', 'skill_load']
  // call_mcp_tool stands for every generic MCP tool: the Host never assumes a
  // generic MCP call is read-only, so the engine must not unlock it.
  const serialTools = [
    'write_file', 'edit_file', 'run_command', 'call_mcp_tool',
    'http_request', 'sqlite_read', 'read_attachment', 'web_read',
  ]
  const settled = { content: [], isError: false }
  installKernelHostTools(
    session,
    [...parallelReaders, ...serialTools].map(definition),
    { settledFor: () => settled },
  )
  const modes = Object.fromEntries(
    session.agent.state.tools.map(tool => [tool.name, tool.executionMode]),
  )
  for (const name of parallelReaders) {
    assert.equal(modes[name], 'parallel', `${name} is a Host-classified independent reader`)
  }
  for (const name of serialTools) {
    assert.equal(modes[name], 'sequential', `${name} must stay a serial barrier`)
  }
})

test('live executors return only the durable Host-settled result', async () => {
  const session = idleSession()
  const settledResults = new Map([
    ['read-a', { content: [{ type: 'text', text: 'durable text' }], isError: false }],
  ])
  installKernelHostTools(session, [definition('read'), definition('write_file')], {
    settledFor: toolCallId => settledResults.get(toolCallId),
  })
  const read = session.agent.state.tools.find(tool => tool.name === 'read')
  assert.equal(read.executionMode, 'parallel')
  assert.deepEqual(await read.execute('read-a'), settledResults.get('read-a'))
  await assert.rejects(
    read.execute('read-b'),
    /no settled Host result/,
    'an unsettled call must never execute locally',
  )
  const write = session.agent.state.tools.find(tool => tool.name === 'write_file')
  assert.equal(write.executionMode, 'sequential')
})
