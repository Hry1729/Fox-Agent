/**
 * Role F harness-security tests.
 *
 * Independent negative cases for S01 (authorization/scope boundaries) and B04
 * (bounded result views + historical read-back honesty), plus the search
 * continuation (B02) and offset-unit (B03) contracts.
 *
 * O-REVIEW-01 / O-F-01 restructure: every case is tagged with a PHASE so a
 * reader can never mistake a defect reproduction for a security pass.
 *
 *   phase = "baseline"   — the assertion reproduces CURRENT defective behaviour.
 *                          A `passed` baseline case means "the defect is still
 *                          present". It is evidence for the owning role to fix,
 *                          NOT a security pass, and must never be counted as one.
 *   phase = "acceptance" — the assertion verifies the frozen CONTRACTS v1.2
 *                          behaviour. A `passed` acceptance case means the
 *                          contract is satisfied at this layer.
 *   status = "not_run"   — the case could not be executed in this environment;
 *                          the reason is recorded per case and never dropped.
 *
 * Machine-readable per-case records are emitted as a single JSON line at the
 * end, so an outer runner's `1 pass / 0 skip` cannot hide a `not_run`.
 *
 * Scope rules:
 *  - This file only IMPORTS implementation modules and asserts observable
 *    behaviour. Implementation defects are reported via F-REQ, not fixed here.
 *  - All fixtures are synthetic canaries inside the isolated per-run tmp
 *    directory. No user secrets, no real credentials, no network egress.
 *
 * Execution: self-executing. Under `node --test` it is spawned as a test file
 * and the top-level assertions decide pass/fail; it can also be run directly
 * with `node harness-security.test.mjs` (no subprocess).
 *
 * Evidence tier: contract / pure-logic + Node directed regression.
 * Not: real-provider, real-Host integration, or GUI.
 */
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { lstat } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { tmpdir } from 'node:os'
import { executeReadOnlyTool } from '../src/read-only-tool-executors.mjs'
import { createReadOnlyTools } from '../src/read-only-tools.mjs'
import {
  modelToolResultContent,
  normalizeToolResultStorage,
  parseToolResultRef,
  readToolResultNavigationView,
  resultIsRetrievable,
  toolResultRef,
} from '../src/tool-view.mjs'
import {
  CANARY_SECRET,
  LINK_BLOCKED_REASON,
  MATCH_COUNT,
  MATCH_LINE,
  OUTSIDE_CANARY,
  createProjectWithCanaries,
  linkCreationBlocked,
  traversalFromRoot,
} from './fixtures/harness-security/make-security-fixtures.mjs'

const cases = []
const cleanups = []

class SkipTest extends Error {}

async function run(phase, id, name, body) {
  const record = { id, phase, name, status: null, detail: null }
  try {
    await body({
      after: (fn) => cleanups.push(fn),
      skip: (reason) => {
        record.status = 'not_run'
        record.detail = reason
        throw new SkipTest(reason)
      },
    })
    if (record.status !== 'not_run') record.status = 'passed'
  } catch (error) {
    if (error instanceof SkipTest) {
      // already recorded
    } else {
      record.status = 'failed'
      record.detail = (error && error.message) || String(error)
    }
  }
  cases.push(record)
  const tag = record.status === 'not_run' ? 'not_run' : record.status === 'failed' ? 'not ok' : 'ok'
  console.log(`${tag} - [${phase}] ${id} ${name}${record.detail ? `\n    ${record.detail}` : ''}`)
}

function needLinks(context) {
  if (linkCreationBlocked !== null) {
    context.skip(`${LINK_BLOCKED_REASON}: ${linkCreationBlocked}`)
  }
}

// ===========================================================================
// S01 — scope boundaries
// ===========================================================================

// --- wrapper layer: the Node read-only plumbing defers to a preflight gate --
// These prove the wrapper OBEYS an allow/block decision and forwards approved
// input. They do NOT prove the real Rust Host authorization: a stub callback
// was supplied. The real-Host cases are S01-HOST-* below and need a slot.

await run('wrapper', 'S01-WRAPPER-01', 'the Node read-only wrapper honours a blocking preflight', async (context) => {
  const { outside, cleanup } = await createProjectWithCanaries()
  context.after(cleanup)

  const decisions = []
  const tools = createReadOnlyTools(async (toolCallId, tool, input) => {
    decisions.push({ toolCallId, tool, input })
    return { decision: 'block', message: 'out-of-scope sentinel' }
  })

  const reader = tools.find((candidate) => candidate.name === 'read')
  assert.ok(reader, 'read tool must be registered')
  await assert.rejects(
    reader.execute('call-1', { path: join(outside, 'leaked.txt') }),
    /out-of-scope sentinel/,
  )
  assert.equal(decisions.length, 1, 'the gate saw exactly one request')
  assert.equal(decisions[0].tool, 'read')
  context.note = 'wrapper plumbing only: a stub gate was used; real Rust Host authorization is S01-HOST-01/02'
})

await run('wrapper', 'S01-WRAPPER-02', 'the Node read-only wrapper executes only approved input', async (context) => {
  const { root, cleanup } = await createProjectWithCanaries()
  context.after(cleanup)

  const seen = []
  const tools = createReadOnlyTools(async (toolCallId, tool, input) => {
    seen.push(input)
    return { decision: 'allow', input, executionRoute: 'runtime' }
  })
  const reader = tools.find((candidate) => candidate.name === 'read')
  const result = await reader.execute('call-2', { path: join(root, 'inside.txt') })
  assert.match(result.content[0].text, /public-data/)
  assert.equal(seen.length, 1, 'preflight ran once and its approved input reached the executor')
  context.note = 'wrapper plumbing only: a stub gate was used'
})

// --- real Rust Host authorization (queued, not a pass) ----------------------

await run('acceptance', 'S01-HOST-NOTRUN-01', 'real Rust Host refuses an out-of-scope read with zero side effects (Legacy)', async (context) => {
  context.skip('requires F-BASELINE-HOST slot: real Host tool.preflight via executeHost, legacy authority; request filed in REPORT-F')
})

await run('acceptance', 'S01-HOST-NOTRUN-02', 'real Rust Host refuses an out-of-scope read with zero side effects (Authoritative)', async (context) => {
  context.skip('requires F-BASELINE-HOST slot: real Host tool.preflight via executeHost, authoritative authority; request filed in REPORT-F')
})

// --- baseline: the Node executor alone enforces nothing ----------------------

await run('baseline', 'S01-BASE-01', 'Node read-only executor reads an absolute path outside the root unchallenged', async (context) => {
  const { outside, cleanup } = await createProjectWithCanaries()
  context.after(cleanup)

  // BASELINE: the defect is that scope protection is 100% delegated to the Host
  // gate and the runtime layer keeps no independent check. A `passed` here means
  // "the gap is still present" — it is not a security pass.
  const result = await executeReadOnlyTool('read', { path: join(outside, 'leaked.txt') })
  assert.equal(result.content[0].text, OUTSIDE_CANARY)
  context.note = 'defect reproduced: runtime layer has no independent scope check (owner A preflight / B executor)'
})

await run('baseline', 'S01-BASE-02', 'parent-directory traversal from the root reads outside data', async (context) => {
  const { root, cleanup } = await createProjectWithCanaries()
  context.after(cleanup)

  const result = await executeReadOnlyTool('read', { path: traversalFromRoot(root) })
  assert.equal(result.content[0].text, OUTSIDE_CANARY)
  context.note = 'defect reproduced: traversal not rejected at runtime layer'
})

await run('baseline', 'S01-NOTRUN-01', 'symlink inside the project escapes to an outside canary', async (context) => {
  needLinks(context)
  const { root, cleanup } = await createProjectWithCanaries()
  context.after(cleanup)
  const link = join(root, 'link-to-outside.txt')
  assert.ok((await lstat(link)).isSymbolicLink(), 'fixture symlink must exist')

  const result = await executeReadOnlyTool('read', { path: link })
  assert.equal(result.content[0].text, OUTSIDE_CANARY)
})

// ===========================================================================
// B03 — read.offset is UTF-16 code units; surrogate-pair safety
// ===========================================================================

await run('acceptance', 'B03-ACC-01', 'a whole-code-unit offset returns the exact range and flags truncation', async (context) => {
  const { mkdir, rm, writeFile } = await import('node:fs/promises')
  const dir = join(tmpdir(), `fox-harness-b03-${process.pid}`)
  await mkdir(dir, { recursive: true })
  context.after(() => rm(dir, { recursive: true, force: true }))

  // 'a😀b': 'a' (1 unit), '😀' (2 surrogate units), 'b' (1 unit) => 4 UTF-16 units.
  const text = 'a😀b'
  await writeFile(join(dir, 'u.txt'), text, 'utf8')

  // offset=1 limit=2 spans the whole astral character without splitting it.
  const atOne = await executeReadOnlyTool('read', { path: join(dir, 'u.txt'), offset: 1, limit: 2 })
  assert.equal(atOne.content[0].text, text.slice(1, 3))
  assert.ok(atOne.details.truncated, 'offset+limit < length must flag truncated')
})

/// CONTRACTS v1.2 §6 (RD): the historical surrogate-pair splitting is a defect,
/// not the contract — "历史拆代理对行为仅能作基线复现，不能据此关闭 B03".
/// Kept as an explicit baseline reproduction so the defect stays visible; a fix
/// by B flips this case, and the acceptance above is what must then hold.
await run('baseline', 'B03-BASE-01', 'an offset landing between surrogates currently splits the pair', async (context) => {
  const { mkdir, rm, writeFile } = await import('node:fs/promises')
  const dir = join(tmpdir(), `fox-harness-b03-${process.pid}`)
  await mkdir(dir, { recursive: true })
  context.after(() => rm(dir, { recursive: true, force: true }))

  const text = 'a😀b'
  await writeFile(join(dir, 'u.txt'), text, 'utf8')

  // offset=2 lands inside the surrogate pair: both layers slice UTF-16 units,
  // so the returned text is a lone surrogate, not a valid character.
  const atTwo = await executeReadOnlyTool('read', { path: join(dir, 'u.txt'), offset: 2, limit: 2 })
  assert.equal(atTwo.content[0].text, text.slice(2, 4))
  context.note = 'defect reproduced: offset can split a surrogate pair (owner B); v1.2 requires a safe boundary'
})

// ===========================================================================
// B02 — match-limit continuation boundary
// ===========================================================================

await run('baseline', 'B02-BASE-01', 'grep stops at maxMatches and exposes no continuation cursor', async (context) => {
  const { root, cleanup } = await createProjectWithCanaries()
  context.after(cleanup)

  const grep = await executeReadOnlyTool('grep', { path: root, pattern: MATCH_LINE })
  const lines = grep.content[0].text.split('\n').filter((line) => line.length > 0)

  assert.equal(grep.details.count, 200, 'default maxMatches cap is 200')
  assert.equal(lines.length, 200, 'emitted lines must equal the cap')
  assert.ok(MATCH_COUNT > 200, 'corpus must exceed the cap for the boundary to be meaningful')

  // BASELINE: the defect is the absence of a resume cursor and of the
  // scanComplete distinction required by CONTRACTS RD-v1.
  assert.equal(grep.details.nextCursor, undefined, 'JS layer offers no resume cursor')
  assert.equal(grep.details.scanComplete, undefined, 'JS layer does not distinguish scanComplete')
  context.note = 'defect reproduced: B02 continuation contract absent at JS layer (owner B)'
})

await run('baseline', 'B02-BASE-02', 'find reports only count and never claims scope exhaustion', async (context) => {
  const { root, cleanup } = await createProjectWithCanaries()
  context.after(cleanup)

  const found = await executeReadOnlyTool('find', { path: root, pattern: 'corpus' })
  assert.equal(found.details.count, 1)
  assert.equal(found.details.scanComplete, undefined)
})

// ===========================================================================
// B04 — bounded result views keep read-back honest
// ===========================================================================

await run('acceptance', 'B04-ACC-01', 'result reference round-trips and parses', () => {
  const ref = toolResultRef('run-1', 'call-1')
  assert.match(ref, /^fox-result:\/\//)
  const parsed = parseToolResultRef(ref)
  assert.equal(parsed?.runId, 'run-1')
  assert.equal(parsed?.toolCallId, 'call-1')

  assert.equal(parseToolResultRef('not-a-reference'), null)
  assert.equal(parseToolResultRef('https://example.com/x'), null)
})

await run('acceptance', 'B04-ACC-02', 'retrievability is decided only by trusted Host storage facts', () => {
  const originalBytes = 1000

  assert.ok(
    resultIsRetrievable({ stored: true, storedBytes: 1000, retrievableBytes: 1000 }, originalBytes),
  )
  assert.ok(
    !resultIsRetrievable({ stored: true, storedBytes: 400, retrievableBytes: 1000 }, originalBytes),
  )
  assert.ok(
    !resultIsRetrievable({ stored: true, storedBytes: 1000, retrievableBytes: 400 }, originalBytes),
  )
  assert.ok(!resultIsRetrievable({ stored: false }, originalBytes))

  // Absent / malformed facts must degrade to "not verified", never to a promise.
  assert.ok(!resultIsRetrievable(null, originalBytes))
  assert.ok(!resultIsRetrievable(undefined, originalBytes))
  assert.ok(!resultIsRetrievable('nope', originalBytes))
  assert.ok(!resultIsRetrievable({}, originalBytes))
})

await run('acceptance', 'B04-ACC-03', 'normalizeToolResultStorage tolerates missing fields without inventing them', () => {
  const full = normalizeToolResultStorage({ stored: true, storedBytes: 10, retrievableBytes: 8, blobSha256: 'abc' })
  assert.deepEqual(full, { stored: true, storedBytes: 10, retrievableBytes: 8, blobSha256: 'abc', failureReason: null })

  const partial = normalizeToolResultStorage({ stored: true })
  assert.equal(partial.stored, true)
  assert.equal(partial.storedBytes, 0)
  assert.equal(partial.retrievableBytes, 0)

  assert.equal(normalizeToolResultStorage(null), null)
  assert.equal(normalizeToolResultStorage('x'), null)
  assert.equal(normalizeToolResultStorage([]), null)
  assert.equal(normalizeToolResultStorage({ stored: 'maybe' }), null)
})

await run('acceptance', 'B04-ACC-04', 'read_tool_result navigation view whitelists only model-relevant facts', () => {
  const details = {
    reference: 'fox-result://run-1/call-1',
    runId: 'run-1',
    toolCallId: 'call-1',
    toolName: 'grep',
    status: 'completed',
    offset: 0,
    returnedBytes: 128,
    nextOffset: 128,
    complete: false,
    originalBytes: 4096,
    truncated: true,
    retrievable: true,
    source: 'settled tool result stored by Fox Host',
    reread: { tool: 'read_tool_result' },
  }

  const nav = readToolResultNavigationView(details)
  assert.deepEqual(nav, {
    reference: 'fox-result://run-1/call-1',
    offset: 0,
    returnedBytes: 128,
    nextOffset: 128,
    complete: false,
    originalBytes: 4096,
    retrievable: true,
    truncated: true,
  })

  for (const field of ['runId', 'toolCallId', 'toolName', 'status', 'source', 'reread']) {
    assert.equal(Object.hasOwn(nav, field), false, `${field} must stay out of the model view`)
  }

  assert.equal(readToolResultNavigationView(null), null)
  assert.equal(readToolResultNavigationView('x'), null)
  assert.equal(readToolResultNavigationView({}), null)
})

await run('acceptance', 'B04-ACC-05', 'modelToolResultContent appends the cursor block only for read_tool_result', () => {
  const details = {
    reference: 'fox-result://run-2/call-9',
    offset: 0,
    returnedBytes: 4,
    nextOffset: 4,
    complete: false,
    originalBytes: 9,
    truncated: true,
    retrievable: true,
  }
  const content = [{ type: 'text', text: 'abcd' }]

  const projected = modelToolResultContent('read_tool_result', { content, details })
  assert.equal(projected.length, 2)
  assert.equal(projected[0].text, 'abcd', 'stored fragment stays first and untouched')
  assert.match(projected[1].text, /^FOX_RESULT_CURSOR_V1 /)

  // Any other tool must not gain a navigation block.
  const other = modelToolResultContent('grep', { content, details })
  assert.equal(other.length, 1)

  // An error result must not project read-back navigation.
  const errored = modelToolResultContent('read_tool_result', { isError: true, content, details })
  assert.equal(errored.length, 1, 'isError path must stay free of navigation')
})

await run('acceptance', 'B04-ACC-06', 'repeated projection of the same range is idempotent', () => {
  const details = {
    reference: 'fox-result://run-3/call-1',
    offset: 0,
    returnedBytes: 4,
    complete: true,
    originalBytes: 4,
    truncated: false,
    retrievable: true,
  }
  const content = [{ type: 'text', text: 'abcd' }]
  const once = modelToolResultContent('read_tool_result', { content, details })
  const twice = modelToolResultContent('read_tool_result', { content: once, details })
  assert.equal(twice.length, 2, 'a second projection must not stack another cursor block')
})

await run('acceptance', 'S01-ACC-03', 'project secret canary stays distinguishable from view metadata', async (context) => {
  const { root, cleanup } = await createProjectWithCanaries()
  context.after(cleanup)

  const read = await executeReadOnlyTool('read', { path: join(root, 'secret-canary.txt') })
  assert.equal(read.content[0].text, CANARY_SECRET)
  const view = modelToolResultContent('read', {
    content: [{ type: 'text', text: read.content[0].text }],
    details: null,
  })
  assert.equal(view[0].text, CANARY_SECRET)
})

// ===========================================================================
// S02 — preview fixtures (F-S02-CONFIG-01)
// ===========================================================================

const PREVIEW_DIR = join(dirname(fileURLToPath(import.meta.url)), 'fixtures', 'harness-security', 'preview')

await run('acceptance', 'S02-ACC-01', 'the legitimate preview fixture references no remote resource', async () => {
  const html = await readFile(join(PREVIEW_DIR, 'legitimate-preview.html'), 'utf8')
  assert.ok(html.includes('<!DOCTYPE html>'), 'fixture must be a standalone document')

  // Inspect actual resource references, not comment text: a bare '//' appears in
  // ordinary inline JS comments and is not a network reference. Only absolute
  // URLs, protocol-relative references and network APIs count.
  const remoteReference = /(?:src|href)\s*=\s*['"]?(?:https?:|\/\/)|(?:url\(|@import\s+['"]?)(?:https?:|\/\/)|fetch\s*\(|XMLHttpRequest|sendBeacon|navigator\.serviceWorker/g
  assert.ok(html.match(remoteReference) === null, `legitimate fixture must not reference remote resources: ${JSON.stringify(html.match(remoteReference))}`)

  assert.ok(html.includes('<style>'), 'local styling keeps style-src usable')
  assert.ok(/<script>[\s\S]*document\.title/.test(html), 'local DOM script is present')
})

await run('acceptance', 'S02-ACC-02', 'the malicious preview fixture enumerates every CSP escape vector', async () => {
  const html = await readFile(join(PREVIEW_DIR, 'malicious-preview.html'), 'utf8')
  assert.ok(html.includes('<!DOCTYPE html>'), 'fixture must be a standalone document')

  // Every vector the proposed CSP is supposed to block must be present, so a
  // later browser-level run cannot silently pass on an incomplete probe.
  for (const vector of ['fetch(', 'sendBeacon', 'img.src', 'window.parent.document', 'createElement(\'script\')', 'requestSubmit']) {
    assert.ok(html.includes(vector), `malicious fixture must probe the vector: ${vector}`)
  }
  for (const directive of ['connect-src', 'img-src', 'script-src', 'form-action']) {
    assert.ok(html.includes(directive), `fixture comment must name the CSP directive it targets: ${directive}`)
  }
})

await run('acceptance', 'S02-ACC-03', 'the verifiable CSP text denies every unenumerated remote source', async () => {
  // The policy that ships with the test lives in the in-repo fixture so the
  // suite is self-contained and does not depend on TASK_PACK documents that
  // are not delivered through Git. The human-readable proposal stays canonical
  // in patches/F/.
  const csp = (await readFile(join(PREVIEW_DIR, 'proposed-csp.txt'), 'utf8')).trim()

  assert.ok(csp.startsWith("default-src 'self'"), 'default-deny baseline')
  assert.ok(csp.includes("script-src 'self'"), 'script-src must be self-only')
  assert.ok(csp.includes("style-src 'self' 'unsafe-inline'"), 'styles may inline; scripts may not')
  assert.ok(!/\bscript-src\b[^;]*unsafe-inline/.test(csp), 'script-src must not carry unsafe-inline')
  assert.ok(csp.includes("object-src 'none'"), 'plugins must be disabled')
  assert.ok(csp.includes('https://models.dev'), 'only enumerated remote image host')
  assert.ok(csp.includes('public.blob.vercel-storage.com'), 'only enumerated remote connect host')
})

// The browser-level blocking test cannot run outside the desktop build.
await run('acceptance', 'S02-NOTRUN-01', 'malicious preview is actually blocked in the real webview', async (context) => {
  context.skip('requires the production desktop build and a queue slot (F-CSP-DESKTOP); the srcdoc CSP-inheritance behaviour must be measured, not assumed')
})

// ===========================================================================
// Machine-readable summary — one JSON line, per case, nothing hidden
// ===========================================================================

for (const cleanup of cleanups) {
  try { await cleanup() } catch { /* best effort */ }
}

const summary = {
  generatedAt: new Date().toISOString(),
  counts: {
    total: cases.length,
    passed: cases.filter((c) => c.status === 'passed').length,
    failed: cases.filter((c) => c.status === 'failed').length,
    notRun: cases.filter((c) => c.status === 'not_run').length,
    acceptancePassed: cases.filter((c) => c.phase === 'acceptance' && c.status === 'passed').length,
    acceptanceFailed: cases.filter((c) => c.phase === 'acceptance' && c.status === 'failed').length,
    baselineReproduced: cases.filter((c) => c.phase === 'baseline' && c.status === 'passed').length,
    baselineFailed: cases.filter((c) => c.phase === 'baseline' && c.status === 'failed').length,
    wrapperPassed: cases.filter((c) => c.phase === 'wrapper' && c.status === 'passed').length,
  },
  reading: {
    acceptance: 'passed == frozen CONTRACTS v1.2 behaviour verified at this layer',
    baseline: 'passed == current DEFECT still present; a fix by the owning role flips this case to failed until the contract acceptance passes',
    wrapper: 'passed == the Node plumbing honours the gate; NOT real Rust Host authorization (a stub callback was supplied)',
    not_run: 'case could not execute here; reason recorded per case',
  },
  cases,
}
console.log('FOX_HARNESS_SUMMARY ' + JSON.stringify(summary))

if (summary.counts.failed > 0) {
  process.exitCode = 1
}
