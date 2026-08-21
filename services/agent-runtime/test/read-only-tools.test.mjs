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
  const tools = createReadOnlyTools(async (tool, input) => {
    preflightCalls.push({ tool, input })
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
    { tool: 'ls', input: { path: '.' } },
    { tool: 'find', input: { pattern: 'README', path: '.' } },
    { tool: 'grep', input: { pattern: 'Fox', path: '.' } },
  ])
})
