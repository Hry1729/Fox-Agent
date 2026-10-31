import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises'
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

test('R4 read version hashes the full raw file and reaches model content on every page', async (context) => {
  const { createHash } = await import('node:crypto')
  const root = await mkdtemp(join(tmpdir(), 'fox-read-version-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  const path = join(root, 'note.txt')
  const bytes = Buffer.from('第一行\r\nsecond 😀\r\n')
  await writeFile(path, bytes)
  const expected = `sha256:${createHash('sha256').update(bytes).digest('hex')}`
  for (const page of [{ offset: 0, limit: 1 }, { startLine: 2, lineCount: 1 }, { startLine: 99, lineCount: 1 }]) {
    const result = await executeReadOnlyTool('read', { path, ...page })
    assert.equal(result.details.readVersion, expected)
    assert.ok(result.content.some(block => block.text.includes(expected)))
  }
})

// Windows 创建符号链接通常需要特权，这里只在非 Windows 上跑。
const skipSymlinks = process.platform === 'win32'

test('rt4_find_and_grep_skip_links_that_leave_the_project', { skip: skipSymlinks }, async (context) => {
  const parent = await mkdtemp(join(tmpdir(), 'fox-rt4-boundary-'))
  const root = join(parent, 'proj')
  const outside = join(parent, 'proj2')
  context.after(() => rm(parent, { recursive: true, force: true }))
  await mkdir(root)
  await mkdir(outside)
  const secret = 'OUTSIDE_ROOT_SENTINEL_rt4'
  await writeFile(join(outside, 'id_rsa'), `${secret}\n`, 'utf8')
  await writeFile(join(root, 'keep.txt'), 'inside keep\n', 'utf8')
  // `proj2` 以 `proj` 为前缀，但不是它的子目录。分隔符边界必须把它挡在外面。
  await symlink(join(outside, 'id_rsa'), join(root, 'leak'))

  const grep = await executeReadOnlyTool('grep', { path: root, pattern: secret })
  assert.equal(grep.content[0].text.includes(secret), false)
  assert.equal(grep.details.skipped.outsideRoot, 1)

  const find = await executeReadOnlyTool('find', { path: root, pattern: 'leak' })
  assert.equal(find.content[0].text.includes('leak'), false)
  assert.equal(find.details.skipped.outsideRoot, 1)

  const kept = await executeReadOnlyTool('find', { path: root, pattern: 'keep' })
  assert.match(kept.content[0].text, /keep\.txt/)
})

test('rt4_links_inside_the_project_are_still_read', { skip: skipSymlinks }, async (context) => {
  const parent = await mkdtemp(join(tmpdir(), 'fox-rt4-inside-'))
  const root = join(parent, 'proj')
  context.after(() => rm(parent, { recursive: true, force: true }))
  await mkdir(join(root, 'src'), { recursive: true })
  await writeFile(join(root, 'src', 'note.txt'), 'INSIDE_LINK_SENTINEL\n', 'utf8')
  await symlink(join(root, 'src', 'note.txt'), join(root, 'alias.txt'))
  await symlink(join(root, 'src'), join(root, 'src-link'))
  await symlink(join(root, 'missing-target'), join(root, 'dangling'))

  const grep = await executeReadOnlyTool('grep', { path: root, pattern: 'INSIDE_LINK_SENTINEL' })
  assert.match(grep.content[0].text, /note\.txt/)
  assert.match(grep.content[0].text, /alias\.txt/)
  assert.equal(grep.content[0].text.includes('src-link'), false)
  assert.equal(grep.details.skipped.outsideRoot, 0)
  assert.equal(grep.details.skipped.brokenLinks, 1)
  assert.equal(grep.details.skipped.linkedDirectories, 1)
  assert.equal(grep.content[0].text.split('\n').filter(Boolean).length, 2)

  const find = await executeReadOnlyTool('find', { path: root, pattern: 'alias' })
  assert.match(find.content[0].text, /alias\.txt/)
  assert.equal(find.details.skipped.outsideRoot, 0)
})
