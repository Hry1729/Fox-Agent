import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { executeReadOnlyTool, sliceUtf16Range } from '../src/read-only-tool-executors.mjs'
import { searchNavigationView, renderSearchNavigation, modelToolResultContent } from '../src/tool-view.mjs'

// ---------------------------------------------------------------------------
// E-T-03b: surrogate-pair safe UTF-16 paging (O-REVIEW-02 item 1)
// ---------------------------------------------------------------------------

const LONE_SURROGATE = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/

test('E-T-03b: offset inside a surrogate pair never returns a lone surrogate', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-03b-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  // 'A'(U+0041) + U+1F600 (D83D DE00) + 'Fox': units [41 D83D DE00 46 6F 78]
  await writeFile(join(root, 'emoji.txt'), 'A😀Fox', 'utf8')
  const path = join(root, 'emoji.txt')
  const result = await executeReadOnlyTool('read', { path, offset: 2, limit: 2 })
  assert.ok(!LONE_SURROGATE.test(result.content[0].text), `no lone surrogate: ${JSON.stringify(result.content[0].text)}`)
  assert.equal(result.details.readMode, 'utf16')
  assert.ok(result.details.nextOffset !== undefined, 'nextOffset must be present')
})

test('UTF-16 pages reassemble exactly via nextOffset (first/middle/last)', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-pages-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  // Mixed BMP + non-BMP + CRLF content.
  const source = 'A😀Fox\r\n中文行😀尾\nline3 plain\n🎉🎉🎉\nend'
  await writeFile(join(root, 'mixed.txt'), source, 'utf8')
  const path = join(root, 'mixed.txt')
  // Paginate with a small limit that forces mid-pair landings.
  let offset = 0
  let reassembled = ''
  let pages = 0
  for (;;) {
    const page = await executeReadOnlyTool('read', { path, offset, limit: 5 })
    assert.ok(!LONE_SURROGATE.test(page.content[0].text), `page ${pages} well-formed`)
    reassembled += page.content[0].text
    pages++
    if (pages > 10_000) throw new Error('paging did not terminate')
    if (page.details.nextOffset === null || page.details.truncated === false) break
    // Advance by the ACTUAL returned end, not offset+limit.
    assert.ok(page.details.nextOffset > offset, 'nextOffset must advance')
    offset = page.details.nextOffset
  }
  assert.equal(reassembled, source, 'reassembly must be exact')
  assert.ok(pages >= 3, `expected >=3 pages, got ${pages}`)
})

test('sliceUtf16Range: direct unit cases', () => {
  // 'A😀Fox' units: A=0, D83D=1, DE00=2, F=3, o=4, x=5
  const s = 'A😀Fox'
  // offset=2 lands on the low half: steps back to include the whole pair.
  const mid = sliceUtf16Range(s, 2, 2)
  assert.ok(!LONE_SURROGATE.test(mid.text))
  assert.ok(mid.end > 2, 'must advance past the pair')
  // O-REVIEW-03 item 1: a too-small window in front of a pair must PROGRESS,
  // not return a dead empty page. limit=1 at the high half extends by one
  // unit to return the complete character.
  const hi = sliceUtf16Range(s, 1, 1)
  assert.equal(hi.text, '😀', 'window extends to one whole character — no dead page')
  assert.equal(hi.end, 3, 'next page continues after the pair')
  // Full read is complete with null nextOffset semantics.
  const full = sliceUtf16Range(s, 0, 100)
  assert.equal(full.text, s)
  assert.equal(full.complete, true)
})

// ---------------------------------------------------------------------------
// RD-v1 field-independence: capped page does not imply incomplete scan
// (O-REVIEW-02 item 4 — do NOT follow the erroneous E checker)
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// B02 cursor: deterministic continuation, scope/query binding, rejection
// ---------------------------------------------------------------------------

test('cursor continues an exhausted-in-one-page scan to the rest (no rescan from zero)', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-cursor-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  for (let i = 0; i < 6; i++) { await writeFile(join(root, `item${i}.txt`), 'x\n', 'utf8') }
  const first = await executeReadOnlyTool('find', { path: root, pattern: 'txt' }, { limits: { maxEntries: 4 } })
  assert.equal(first.details.scanComplete, false, 'first page must not claim exhaustion')
  assert.ok(typeof first.details.nextCursor === 'string' && first.details.nextCursor.length > 0, 'must issue a cursor')
  const second = await executeReadOnlyTool('find', { path: root, pattern: 'txt', cursor: first.details.nextCursor }, { limits: { maxEntries: 4 } })
  const seen = new Set([...first.content[0].text.split('\n'), ...second.content[0].text.split('\n')].filter(Boolean))
  assert.equal(seen.size, 6, 'both pages together cover all 6 files with no overlap loss')
  assert.equal(second.details.nextCursor, null, 'exhausted scope yields null cursor')
})

test('cursor rejects wrong scope, wrong page size and garbage', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-cursor-neg-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'a.txt'), 'x\n', 'utf8')
  const other = await mkdtemp(join(tmpdir(), 'fox-cursor-other-'))
  context.after(() => rm(other, { recursive: true, force: true }))
  await writeFile(join(other, 'b.txt'), 'x\n', 'utf8')
  const first = await executeReadOnlyTool('find', { path: root, pattern: 'txt' }, { limits: { maxEntries: 4 } })
  // nextCursor is null here (exhausted); craft continuation manually via a real cursor:
  const big = await mkdtemp(join(tmpdir(), 'fox-cursor-big-'))
  context.after(() => rm(big, { recursive: true, force: true }))
  for (let i = 0; i < 6; i++) { await writeFile(join(big, `f${i}.txt`), 'x\n', 'utf8') }
  const page1 = await executeReadOnlyTool('find', { path: big, pattern: 'txt' }, { limits: { maxEntries: 4 } })
  const cursor = page1.details.nextCursor
  assert.ok(cursor, 'need a live cursor for rejection tests')
  await assert.rejects(
    executeReadOnlyTool('find', { path: other, pattern: 'txt', cursor }, { limits: { maxEntries: 4 } }),
    /scope mismatch/,
  )
  await assert.rejects(
    executeReadOnlyTool('find', { path: big, pattern: 'txt', cursor }, { limits: { maxEntries: 99 } }),
    /scope mismatch/,
  )
  // A different query for the same scope must also be rejected, not answered.
  await assert.rejects(
    executeReadOnlyTool('find', { path: big, pattern: 'f1', cursor }, { limits: { maxEntries: 4 } }),
    /scope mismatch/,
  )
  await assert.rejects(
    executeReadOnlyTool('find', { path: big, pattern: 'txt', caseSensitive: true, cursor }, { limits: { maxEntries: 4 } }),
    /scope mismatch/,
  )
  await assert.rejects(
    executeReadOnlyTool('find', { path: big, pattern: 'txt', cursor: '!!!not-a-cursor!!!' }, { limits: { maxEntries: 4 } }),
    /invalid cursor/,
  )
  void first
})

test('grep cursor continuation covers all files across pages', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-grep-cursor-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  for (let i = 0; i < 5; i++) { await writeFile(join(root, `g${i}.txt`), `needle-${i}\n`, 'utf8') }
  const first = await executeReadOnlyTool('grep', { path: root, pattern: 'needle' }, { limits: { maxEntries: 3 } })
  assert.ok(first.details.nextCursor, 'must issue a cursor when scope remains')
  const second = await executeReadOnlyTool('grep', { path: root, pattern: 'needle', cursor: first.details.nextCursor }, { limits: { maxEntries: 3 } })
  const all = [...first.content[0].text.split('\n'), ...second.content[0].text.split('\n')].filter(Boolean)
  assert.equal(all.length, 5, 'all 5 hits reachable across pages')
})

test('complete scan with capped hits: scanComplete=true + matchLimitReached=true', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-cap-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  for (let i = 0; i < 5; i++) { await writeFile(join(root, `m${i}.txt`), 'x\n', 'utf8') }
  const result = await executeReadOnlyTool('find', { path: root, pattern: 'txt' }, { limits: { maxMatches: 3 } })
  assert.equal(result.details.scanComplete, true, 'scope exhausted')
  assert.equal(result.details.matchLimitReached, true, 'hits hit the cap')
  assert.equal(result.details.pageComplete, false, 'retained matches mean the page is incomplete')
  assert.equal(result.details.totalMatches, null, 'unknown total stays null')
  assert.equal(result.details.returnedCount, 3)
})


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

test('one-unit emoji read advances and final output cap never splits a surrogate pair', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b03-budget-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  const path = join(root, 'emoji.txt')
  await writeFile(path, 'A😀tail', 'utf8')
  const tiny = await executeReadOnlyTool('read', { path, offset: 1, limit: 1 })
  assert.equal(tiny.content[0].text, '😀')
  assert.equal(tiny.details.nextOffset, 3)
  const capped = await executeReadOnlyTool('read', { path, offset: 0, limit: 7 }, { limits: { maxOutputChars: 2 } })
  assert.equal(capped.content[0].text, 'A')
  assert.equal(capped.details.returnedUnits, 1)
  assert.equal(capped.details.nextOffset, 1)
  assert.doesNotMatch(capped.content[0].text, LONE_SURROGATE)
})

test('find pagination retains matches scanned beyond match and entry caps', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b02-caps-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  for (let i = 0; i < 5; i++) await writeFile(join(root, `hit-${i}.txt`), 'hit', 'utf8')
  const seen = new Set()
  let cursor = null
  for (let page = 0; page < 10; page++) {
    const result = await executeReadOnlyTool('find', { path: root, pattern: 'hit', cursor }, { limits: { maxEntries: 4, maxMatches: 2 } })
    result.content[0].text.split('\n').filter(Boolean).forEach((path) => seen.add(path))
    if (result.details.matchLimitReached) assert.equal(result.details.pageComplete, false)
    cursor = result.details.nextCursor
    if (!cursor) break
  }
  assert.equal(seen.size, 5)
})

test('grep continues within one file after match cap and applies glob', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b02-grep-cap-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(join(root, 'yes.txt'), 'hit\nhit\nhit\nhit\nhit', 'utf8')
  await writeFile(join(root, 'no.md'), 'hit', 'utf8')
  const seen = new Set()
  let cursor = null
  for (let page = 0; page < 10; page++) {
    const result = await executeReadOnlyTool('grep', { path: root, pattern: 'hit', glob: '*.txt', cursor }, { limits: { maxMatches: 2 } })
    result.content[0].text.split('\n').filter(Boolean).forEach((line) => seen.add(line))
    assert.equal(result.content[0].text.includes('no.md'), false)
    cursor = result.details.nextCursor
    if (!cursor) break
  }
  assert.equal(seen.size, 5)
})

test('cursor binds authorization scope and declares live non-snapshot semantics', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b02-auth-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  for (let i = 0; i < 5; i++) await writeFile(join(root, `f${i}.txt`), 'x', 'utf8')
  const first = await executeReadOnlyTool('find', { path: root, pattern: 'txt' }, {
    limits: { maxEntries: 2 }, authorizationScope: 'permission-A',
  })
  assert.equal(first.details.cursorConsistency, 'live')
  assert.equal(first.details.cursorStalePossible, true)
  await assert.rejects(
    executeReadOnlyTool('find', { path: root, pattern: 'txt', cursor: first.details.nextCursor }, {
      limits: { maxEntries: 2 }, authorizationScope: 'permission-B',
    }),
    /cursor scope mismatch/,
  )
})

test('bounded line mode exposes UTF-16 continuation for a long single line', async (context) => {
  const root = await mkdtemp(join(tmpdir(), 'fox-b03-long-line-'))
  context.after(() => rm(root, { recursive: true, force: true }))
  const path = join(root, 'long.txt')
  await writeFile(path, 'A😀tail', 'utf8')
  const page = await executeReadOnlyTool('read', { path, startLine: 1, lineCount: 1 }, { limits: { maxOutputChars: 2 } })
  assert.equal(page.content[0].text, 'A')
  assert.equal(page.details.pageComplete, false)
  assert.equal(page.details.nextStartLine, null)
  assert.equal(page.details.nextOffset, 1)
  assert.equal(page.details.returnedUnits, 1)
})
