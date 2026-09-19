import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { executeReadOnlyTool } from '../src/read-only-tool-executors.mjs'
import { searchNavigationView, renderSearchNavigation, modelToolResultContent } from '../src/tool-view.mjs'

// ---------------------------------------------------------------------------
// B01: search filtering, scope and pattern completeness
// ---------------------------------------------------------------------------

test('find/grep skip dependency noise (node_modules, target, .git)', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b01-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await mkdir(join(root, 'src'), { recursive: true })
  await mkdir(join(root, 'node_modules', 'pkg'), { recursive: true })
  await mkdir(join(root, 'target', 'debug'), { recursive: true })
  await mkdir(join(root, '.git', 'objects'), { recursive: true })
  await writeFile(join(root, 'src', 'app.js'), 'const x = 1\n', 'utf8')
  await writeFile(join(root, 'node_modules', 'pkg', 'index.js'), 'module.exports = {}\n', 'utf8')
  await writeFile(join(root, 'target', 'debug', 'build.rs'), 'fn main() {}\n', 'utf8')
  await writeFile(join(root, '.git', 'HEAD'), 'ref: refs/heads/main\n', 'utf8')

  const findResult = await executeReadOnlyTool('find', { path: root, pattern: 'js' })
  const paths = findResult.content[0].text.split('\n').filter(Boolean)
  assert.ok(paths.every((p) => !p.includes('node_modules')), 'find must not return node_modules')
  assert.ok(paths.every((p) => !p.includes('target')), 'find must not return target')
  assert.ok(paths.every((p) => !p.includes('.git')), 'find must not return .git')
  assert.equal(findResult.details.skippedIgnored > 0, true, 'must report skipped ignored entries')

  const grepResult = await executeReadOnlyTool('grep', { path: root, pattern: 'const' })
  assert.ok(grepResult.content[0].text.includes('src'), 'grep must find src file')
  assert.ok(!grepResult.content[0].text.includes('node_modules'), 'grep must not return node_modules')
})

test('find supports caseSensitive option', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b01-case-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'README.md'), '# Hello\n', 'utf8')
  await writeFile(join(root, 'readme.txt'), 'hello\n', 'utf8')

  const insensitive = await executeReadOnlyTool('find', { path: root, pattern: 'readme' })
  assert.equal(insensitive.details.count, 2)

  const sensitive = await executeReadOnlyTool('find', { path: root, pattern: 'readme', caseSensitive: true })
  assert.equal(sensitive.details.count, 1)
  assert.ok(sensitive.content[0].text.includes('readme.txt'))
})

test('find supports glob option', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b01-glob-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'app.ts'), 'export {}\n', 'utf8')
  await writeFile(join(root, 'app.js'), 'module.exports = {}\n', 'utf8')
  await writeFile(join(root, 'style.css'), 'body{}\n', 'utf8')

  const tsOnly = await executeReadOnlyTool('find', { path: root, pattern: 'app', glob: '*.ts' })
  assert.equal(tsOnly.details.count, 1)
  assert.ok(tsOnly.content[0].text.includes('app.ts'))

  const allJs = await executeReadOnlyTool('find', { path: root, pattern: 'app', glob: '*.js' })
  assert.equal(allJs.details.count, 1)
  assert.ok(allJs.content[0].text.includes('app.js'))
})

// B02: pagination completeness metadata
test('find distinguishes scanComplete from matchLimitReached', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b02-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'a.txt'), 'alpha\n', 'utf8')
  await writeFile(join(root, 'b.txt'), 'beta\n', 'utf8')
  await writeFile(join(root, 'c.txt'), 'gamma\n', 'utf8')
  const result = await executeReadOnlyTool('find', { path: root, pattern: 'txt' })
  assert.equal(result.details.scanComplete, true)
  assert.equal(result.details.pageComplete, true)
  assert.equal(result.details.matchLimitReached, false)
  assert.equal(result.details.totalMatches, 3)
})

test('find reports null totalMatches when scan incomplete', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b02-null-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  for (let i = 0; i < 10; i++) { await writeFile(join(root, `file${i}.txt`), `c${i}\n`, 'utf8') }
  const result = await executeReadOnlyTool('find', { path: root, pattern: 'txt' }, { limits: { maxEntries: 5 } })
  assert.equal(result.details.scanComplete, false)
  assert.equal(result.details.totalMatches, null)
})

test('grep zero-match distinguishes complete from incomplete scan', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b02-zero-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'a.txt'), 'hello world\n', 'utf8')
  const complete = await executeReadOnlyTool('grep', { path: root, pattern: 'zzzzz' })
  assert.equal(complete.details.count, 0)
  assert.equal(complete.details.scanComplete, true)
  assert.equal(complete.details.totalMatches, 0)
  const incomplete = await executeReadOnlyTool('grep', { path: root, pattern: 'zzzzz' }, { limits: { maxEntries: 1 } })
  assert.equal(incomplete.details.scanComplete, false)
  assert.equal(incomplete.details.totalMatches, null)
})

// B03: line-mode read parameters
test('read with startLine/lineCount returns correct range', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b03-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'test.txt'), 'line1\nline2\nline3\nline4\nline5\n', 'utf8')
  const result = await executeReadOnlyTool('read', { path: join(root, 'test.txt'), startLine: 2, lineCount: 3 })
  assert.equal(result.content[0].text, 'line2\nline3\nline4\n')
  assert.equal(result.details.startLine, 2)
  assert.equal(result.details.totalLines, 5)
  assert.equal(result.details.readMode, 'lines')
})


test('read line mode requires paired positive integers', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b03-strict-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'a.txt'), 'one\ntwo\n', 'utf8')
  const path = join(root, 'a.txt')
  await assert.rejects(executeReadOnlyTool('read', { path, startLine: 1 }), /positive integers/)
  await assert.rejects(executeReadOnlyTool('read', { path, lineCount: 2 }), /positive integers/)
  await assert.rejects(executeReadOnlyTool('read', { path, startLine: -1, lineCount: 1 }), /positive integers/)
  await assert.rejects(executeReadOnlyTool('read', { path, startLine: 1, lineCount: 1.5 }), /positive integers/)
  await assert.rejects(executeReadOnlyTool('read', { path, startLine: 0, lineCount: 0 }), /positive integers/)
})

test('a bounded line page does not claim completion while discarding requested text', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b03-cut-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'text.txt'), 'one\ntwo\n', 'utf8')
  const result = await executeReadOnlyTool('read', { path: join(root, 'text.txt'), startLine: 1, lineCount: 2 }, { limits: { maxReadChars: 2 } })
  assert.equal(result.details.truncated, true)
  assert.equal(result.details.pageComplete, false)
})

test('read with startLine/lineCount handles CRLF correctly', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b03-crlf-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'crlf.txt'), 'first\r\nsecond\r\nthird\r\n', 'utf8')
  const result = await executeReadOnlyTool('read', { path: join(root, 'crlf.txt'), startLine: 1, lineCount: 2 })
  assert.equal(result.content[0].text, 'first\r\nsecond\r\n')
  assert.equal(result.details.totalLines, 3)
})

test('read rejects mixed offset/limit and startLine/lineCount', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b03-mix-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'a.txt'), 'hello\n', 'utf8')
  await assert.rejects(executeReadOnlyTool('read', { path: join(root, 'a.txt'), offset: 0, startLine: 1 }), /mutually exclusive/)
  await assert.rejects(executeReadOnlyTool('read', { path: join(root, 'a.txt'), limit: 100, lineCount: 5 }), /mutually exclusive/)
})

test('read with unicode in line mode preserves multi-byte characters', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b03-uni-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'u.txt'), '中文第一行\n😀 emoji\n日本語\n', 'utf8')
  const result = await executeReadOnlyTool('read', { path: join(root, 'u.txt'), startLine: 1, lineCount: 3 })
  assert.equal(result.content[0].text, '中文第一行\n😀 emoji\n日本語\n')
  assert.ok(!result.content[0].text.includes('\uFFFD'))
})

// B02: search navigation projection
test('search nav metadata is projected into model view content', () => {
  const details = { count: 5, scanComplete: false, matchLimitReached: true, skippedIgnored: 12, totalMatches: null, pageComplete: true }
  const content = [{ type: 'text', text: 'src/a.ts\nsrc/b.ts' }]
  const view = modelToolResultContent('find', { content, details })
  assert.equal(view.length, 2)
  const nav = JSON.parse(view[1].text.slice('FOX_SEARCH_NAV_V1 '.length))
  assert.equal(nav.scanComplete, false)
  assert.equal(nav.matchLimitReached, true)
  assert.equal(nav.totalMatches, null)
})

test('read nav metadata is projected into model view content', () => {
  const details = { truncated: true, readMode: 'lines', startLine: 1, lineCount: 10, totalLines: 50 }
  const content = [{ type: 'text', text: 'line1\nline2' }]
  const view = modelToolResultContent('read', { content, details })
  assert.equal(view.length, 2)
  const nav = JSON.parse(view[1].text.slice('FOX_SEARCH_NAV_V1 '.length))
  assert.equal(nav.readMode, 'lines')
  assert.equal(nav.totalLines, 50)
})

test('non-search tools do not get search navigation block', () => {
  const view = modelToolResultContent('office_read', { content: [{ type: 'text', text: 'r' }], details: { count: 1 } })
  assert.equal(view.length, 1)
})

test('search nav view filters unlisted fields', () => {
  const nav = searchNavigationView({ count: 1, scanComplete: true, secretField: 'x' })
  assert.ok(!('secretField' in nav))
  assert.equal(nav.count, 1)
})

test('search nav render is parseable', () => {
  const nav = { count: 3, scanComplete: true, totalMatches: 3 }
  const parsed = JSON.parse(renderSearchNavigation(nav).slice('FOX_SEARCH_NAV_V1 '.length))
  assert.deepEqual(parsed, nav)
})

test('grep supports regex option', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b01-regex-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'a.txt'), 'foo123bar\nfoo456bar\nbaz\n', 'utf8')

  const regex = await executeReadOnlyTool('grep', { path: root, pattern: 'foo\\d+bar', regex: true })
  assert.equal(regex.details.count, 2)

  const literal = await executeReadOnlyTool('grep', { path: root, pattern: 'foo\\d+bar' })
  assert.equal(literal.details.count, 0, 'literal search must not match regex pattern')
})
