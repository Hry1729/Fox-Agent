import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { executeReadOnlyTool } from '../src/read-only-tool-executors.mjs'
import { createReadOnlyTools } from '../src/read-only-tools.mjs'

test('executes Fox read, ls, find and grep implementations', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-read-tools-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await mkdir(join(root, 'src'))
  await writeFile(join(root, 'src', 'note.txt'), 'hello Fox\nsecond line', 'utf8')

  const read = await executeReadOnlyTool('read', { path: join(root, 'src', 'note.txt') })
  assert.match(read.content[0].text, /hello Fox/)
  const ls = await executeReadOnlyTool('ls', { path: root })
  assert.match(ls.content[0].text, /\[dir\] src/)
  const find = await executeReadOnlyTool('find', { path: root, pattern: 'note' })
  assert.match(find.content[0].text, /note\.txt/)
  const grep = await executeReadOnlyTool('grep', { path: root, pattern: 'second' })
  assert.match(grep.content[0].text, /note\.txt:2:second line/)
})

test('defaults omitted project-search paths to the authorized project root', async () => {
  const preflightCalls = []
  const tools = createReadOnlyTools(async (toolCallId, tool, input) => {
    preflightCalls.push({ toolCallId, tool, input })
    return { decision: 'block', message: 'preflight sentinel' }
  })
  const cases = [
    { name: 'ls', input: {} },
    { name: 'find', input: { pattern: 'README' } },
    { name: 'grep', input: { pattern: 'Fox' } },
  ]

  for (const entry of cases) {
    const tool = tools.find((candidate) => candidate.name === entry.name)
    assert.ok(tool, `missing ${entry.name} tool`)
    await assert.rejects(
      tool.execute(`call-${entry.name}`, entry.input),
      /preflight sentinel/,
    )
  }

  assert.deepEqual(preflightCalls, [
    { toolCallId: 'call-ls', tool: 'ls', input: { path: '.' } },
    { toolCallId: 'call-find', tool: 'find', input: { pattern: 'README', path: '.' } },
    { toolCallId: 'call-grep', tool: 'grep', input: { pattern: 'Fox', path: '.' } },
  ])
})
test('Rust reader routing keeps the frozen identity and never falls back on failure', async () => {
  const calls = []
  const preflight = async (_id, _tool, input) => ({ decision: 'allow', input, executionRoute: 'rust', permissionSnapshotId: 'sha256:frozen' })
  const tools = createReadOnlyTools(preflight, { executeHost: async (...args) => {
    calls.push(args)
    return { type: 'tool.execute_completed', payload: { isError: false, result: { content: [{ type: 'text', text: 'host result' }], details: {} } } }
  } })
  const read = tools.find(tool => tool.name === 'read')
  const result = await read.execute('reader-1', { path: 'does-not-exist.txt' })
  assert.equal(result.content[0].text, 'host result')
  assert.equal(calls[0][0], 'tool.readonly_execute')
  assert.equal(calls[0][1].permissionSnapshotId, 'sha256:frozen')
  assert.equal(calls[0][1].toolCallId, 'reader-1')
  const failed = createReadOnlyTools(preflight, { executeHost: async () => ({type:'tool.execute_failed',payload:{isError:true,error:'gateway rejected'}}) })
  await assert.rejects(failed.find(tool => tool.name === 'read').execute('reader-2', {path:'does-not-exist.txt'}), /gateway rejected/)
  const unknown = createReadOnlyTools(async () => ({decision:'allow',input:{path:'.'},executionRoute:'unknown'}))
  await assert.rejects(unknown.find(tool => tool.name === 'ls').execute('reader-3', {}), /Unknown frozen/)
})
