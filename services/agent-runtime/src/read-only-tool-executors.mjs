import { readFile, readdir } from 'node:fs/promises'

const DEFAULT_LIMITS = Object.freeze({
  maxReadChars: 120_000,
  maxMatches: 200,
  maxEntries: 4_000,
  maxOutputChars: 120_000,
  maxLineChars: 8_000,
})
const MAX_REGEX_PATTERN_BYTES = 4 * 1024

// ---------------------------------------------------------------------------
// Ignore rules (B01): dependency noise, build output, VCS internals.
// Skipped during walk so they do not consume the scan budget.
// ---------------------------------------------------------------------------

const IGNORED_NAMES = new Set([
  'node_modules', '.git', '.hg', '.svn',
  'target', 'dist', 'build', 'out', '.output',
  '__pycache__', '.pytest_cache', '.mypy_cache',
  '.next', '.nuxt', '.svelte-kit',
  'coverage', '.nyc_output', '.turbo', '.cache',
  'vendor', 'third_party', 'third-party',
])

function isIgnored(name) {
  return IGNORED_NAMES.has(name)
}

// ---------------------------------------------------------------------------
// Search options (B01): case sensitivity, literal/regex, glob.
// ---------------------------------------------------------------------------

function compilePattern(pattern, { caseSensitive = false, regex = false } = {}) {
  if (regex) {
    if (Buffer.byteLength(String(pattern), 'utf8') > MAX_REGEX_PATTERN_BYTES) {
      throw new Error(`invalid regex: pattern exceeds ${MAX_REGEX_PATTERN_BYTES} bytes`)
    }
    const flags = caseSensitive ? '' : 'i'
    let compiled
    try {
      compiled = new RegExp(pattern, flags)
    } catch (error) {
      throw new Error(`invalid regex: ${error instanceof Error ? error.message : String(error)}`)
    }
    return { test: (value) => compiled.test(value) }
  }
  const needle = caseSensitive ? String(pattern) : String(pattern).toLowerCase()
  return {
    test: (value) => caseSensitive ? value.includes(needle) : value.toLowerCase().includes(needle),
  }
}

/** Simple glob: `*` matches any sequence, `?` matches one character. */
function compileGlob(glob) {
  if (!glob || typeof glob !== 'string') return null
  const escaped = glob.replace(/[.+^${}()|[\]\\]/g, '\\$&')
  const pattern = escaped.replace(/\*/g, '.*').replace(/\?/g, '.')
  const re = new RegExp(`^${pattern}$`, 'i')
  return { test: (name) => re.test(name) }
}

function boundedInteger(value, fallback, maximum) {
  const parsed = Number(value)
  if (!Number.isInteger(parsed) || parsed < 1) return fallback
  return Math.min(parsed, maximum)
}

function normalizedLimits(input = {}) {
  return {
    maxReadChars: boundedInteger(input.maxReadChars, DEFAULT_LIMITS.maxReadChars, DEFAULT_LIMITS.maxReadChars),
    maxMatches: boundedInteger(input.maxMatches, DEFAULT_LIMITS.maxMatches, DEFAULT_LIMITS.maxMatches),
    maxEntries: boundedInteger(input.maxEntries, DEFAULT_LIMITS.maxEntries, DEFAULT_LIMITS.maxEntries),
    maxOutputChars: boundedInteger(input.maxOutputChars, DEFAULT_LIMITS.maxOutputChars, DEFAULT_LIMITS.maxOutputChars),
    maxLineChars: boundedInteger(input.maxLineChars, DEFAULT_LIMITS.maxLineChars, DEFAULT_LIMITS.maxLineChars),
  }
}

function textResult(text, details = {}, maxOutputChars = DEFAULT_LIMITS.maxOutputChars) {
  const source = String(text || '')
  // The final output budget is surrogate-pair safe too (O-REVIEW-03 item 2):
  // a character-level cut here previously split a pair and detached the
  // navigation from the actually returned text.
  const bounded = sliceUtf16Budget(source, maxOutputChars)
  return {
    content: [{ type: 'text', text: bounded }],
    details: { ...details, outputTruncated: details.outputTruncated === true || bounded.length < source.length },
  }
}

function throwIfAborted(signal) {
  if (signal?.aborted) throw new Error('Tool execution was cancelled.')
}

// ---------------------------------------------------------------------------
// UTF-16 code-unit slicing with surrogate-pair safety (RD-v1 B03).
// Units stay UTF-16 (contract); boundaries never split a surrogate pair.
// Returns { text, end, complete }: `end` is the exclusive UTF-16 end index
// actually returned, so callers advance offset by real length (no overlap,
// no gap).
// ---------------------------------------------------------------------------

function isHighSurrogate(unit) {
  return unit >= 0xd800 && unit <= 0xdbff
}

function isLowSurrogate(unit) {
  return unit >= 0xdc00 && unit <= 0xdfff
}

export function sliceUtf16Range(text, offset, limit) {
  const total = text.length
  let start = Math.max(0, Math.min(offset, total))
  // Never start inside a pair: if start lands on the low half of a surrogate
  // pair, step back one unit so the pair is returned whole. (Sequential
  // callers that advance by the previous `end` never land mid-pair, but a
  // direct offset can.)
  if (start > 0 && start < total && isHighSurrogate(text.charCodeAt(start - 1)) && isLowSurrogate(text.charCodeAt(start))) {
    start -= 1
  }
  // Clamp the requested end into the string.
  let safeEnd = Math.max(start, Math.min(start + Math.max(0, limit), total))
  // Never end inside a pair: a trailing high surrogate whose low half is
  // outside the window is dropped, so no lone surrogate is ever returned.
  if (safeEnd > start && safeEnd < total && isHighSurrogate(text.charCodeAt(safeEnd - 1)) && isLowSurrogate(text.charCodeAt(safeEnd))) {
    safeEnd -= 1
  }
  // Defensive: a well-formed JS string never ends with a lone high surrogate;
  // if the source is somehow malformed, still never return it alone.
  if (safeEnd > start && safeEnd === total && isHighSurrogate(text.charCodeAt(safeEnd - 1))) {
    safeEnd -= 1
  }
  // O-REVIEW-03 item 1: a window too small to hold even one whole character
  // (e.g. limit=1 in front of a surrogate pair) previously produced an EMPTY
  // page with nextOffset unchanged — an infinite loop. Guaranteed progress:
  // extend the window just enough to return one complete character (at most
  // one extra unit). The window may then exceed `limit` by one unit, which is
  // documented: progress beats a dead page, and units stay UTF-16.
  if (safeEnd === start && start < total) {
    safeEnd = Math.min(start + 2, total)
    if (isHighSurrogate(text.charCodeAt(safeEnd - 1)) && safeEnd < total) {
      safeEnd += 1
    }
  }
  return { text: text.slice(start, safeEnd), start, end: safeEnd, complete: safeEnd >= total }
}

/**
 * Surrogate-pair-safe prefix cut to a UTF-16 unit budget. A trailing high
 * surrogate whose low half would be cut is dropped, so the result is always
 * well-formed. Callers derive navigation from the RETURNED length.
 */
export function sliceUtf16Budget(text, maxUnits) {
  if (text.length <= maxUnits) return text
  let end = Math.max(0, maxUnits)
  if (end > 0 && end < text.length && isHighSurrogate(text.charCodeAt(end - 1)) && isLowSurrogate(text.charCodeAt(end))) {
    end -= 1
  }
  if (end > 0 && end === text.length && isHighSurrogate(text.charCodeAt(end - 1))) {
    end -= 1
  }
  return text.slice(0, end)
}

async function walk(root, { signal, maxEntries = 4_000, glob = null, cursor = null, cursorBinding = null } = {}) {
  const globMatcher = compileGlob(glob)
  // RD-v1 cursor (B02): deterministic continuation of the SAME bound query.
  // The opaque token encodes { binding, consumed, pageSize }: `binding` is a
  // fingerprint of the query (scope root + pattern + options + page size) and
  // `consumed` is how many entries were already returned. A cursor whose
  // binding differs from the current call is rejected — wrong scope, wrong
  // pattern/options or a different page size all fail loudly instead of
  // silently answering a different question.
  // Position, not content hash: renames during pagination may shift results;
  // the cursor guarantees resume-without-rescan, not snapshot isolation.
  const binding = cursorBinding ?? String(root)
  let consumed = 0
  if (cursor !== null && cursor !== undefined) {
    const parsed = decodeCursor(cursor)
    if (!parsed.ok) throw new Error(`invalid cursor: ${parsed.reason}`)
    if (parsed.binding !== binding) {
      throw new Error('cursor scope mismatch: this cursor was issued for a different scope, query or page size')
    }
    consumed = parsed.consumed
  }
  const found = []
  const skipped = { ignored: 0 }
  const queue = [root]
  // Breadth-first order is deterministic for a fixed tree: readdir order is
  // the platform order, and the cursor counts consumed entries in that same
  // order, so continuation resumes exactly where the previous page stopped.
  // Each returned entry carries its `walkIndex` (position in that order) so
  // callers can point the next cursor at the last DELIVERED match instead of
  // the last scanned entry (O-REVIEW-03 item 3: scanned-but-undelivered
  // matches must stay reachable).
  let index = 0
  let lastIndex = 0
  while (queue.length > 0 && found.length < maxEntries) {
    throwIfAborted(signal)
    const directory = queue.shift()
    const entries = await readdir(directory, { withFileTypes: true })
    for (const entry of entries) {
      if (isIgnored(entry.name)) { skipped.ignored++; continue }
      const path = `${directory.replace(/[\\\\/]$/, '')}/${entry.name}`
      if (globMatcher && !entry.isDirectory() && !globMatcher.test(entry.name)) continue
      if (index < consumed) { index++; if (entry.isDirectory()) queue.push(path); continue }
      index++
      lastIndex = index
      found.push({ path, name: entry.name, directory: entry.isDirectory(), walkIndex: index })
      if (entry.isDirectory()) queue.push(path)
      if (found.length >= maxEntries) break
    }
  }
  const exhausted = queue.length === 0 && found.length < maxEntries
  return { entries: found, skipped, scanComplete: exhausted, lastIndex }
}

/** Query fingerprint: everything a cursor must stay bound to. */
export function searchCursorBinding(root, { tool, pattern, caseSensitive = false, regex = false, glob = '', pageSize, authorizationScope = '' }) {
  return [
    String(root), tool, String(pattern ?? ''),
    caseSensitive ? 'cs' : 'ci', regex ? 're' : 'li',
    glob ? `g:${glob}` : 'g:-', `n:${pageSize}`,
    authorizationScope ? `a:${authorizationScope}` : 'a:direct',
  ].join('\u0001')
}

// Opaque cursor token: base64url(JSON { v:1, binding, consumed, pageSize }).
// Opaque to callers; validated on decode. `binding` is compared for equality
// only — it is never an authorization basis, and Host scope checks still apply.
function encodeCursor({ binding, consumed, pageSize }) {
  const payload = JSON.stringify({ v: 1, binding, consumed, pageSize })
  return Buffer.from(payload, 'utf8').toString('base64url')
}

function decodeCursor(cursor) {
  if (typeof cursor !== 'string' || cursor.length === 0 || cursor.length > 4096) {
    return { ok: false, reason: 'cursor must be a non-empty bounded string' }
  }
  let payload
  try {
    payload = JSON.parse(Buffer.from(cursor, 'base64url').toString('utf8'))
  } catch {
    return { ok: false, reason: 'cursor is not decodable' }
  }
  if (!payload || payload.v !== 1 || typeof payload.binding !== 'string' || payload.binding.length === 0) {
    return { ok: false, reason: 'cursor has unknown version or binding' }
  }
  // `consumed` is tool-shaped: find uses a non-negative integer entry
  // position; grep uses { f: fileIndex, l: lineIndexAfterLastDelivered }.
  // Shape validation beyond "present and sane" is the caller's job.
  const consumed = payload.consumed
  const integerOk = Number.isInteger(consumed) && consumed >= 0
  const objectOk = consumed !== null && typeof consumed === 'object'
      && Number.isInteger(consumed.f) && consumed.f >= 0
      && Number.isInteger(consumed.l) && consumed.l >= 0
  if (!integerOk && !objectOk) {
    return { ok: false, reason: 'cursor has an invalid position' }
  }
  if (!Number.isInteger(payload.pageSize) || payload.pageSize < 1) {
    return { ok: false, reason: 'cursor has an invalid page size' }
  }
  return { ok: true, binding: payload.binding, consumed, pageSize: payload.pageSize }
}

export async function executeReadOnlyTool(tool, input, { signal, limits: requestedLimits, authorizationScope = '' } = {}) {
  const limits = normalizedLimits(requestedLimits)
  throwIfAborted(signal)
  if (tool === 'read') {
    const contents = await readFile(input.path, 'utf8')
    // B03: line-mode parameters are mutually exclusive with offset/limit.
    const hasLineMode = input.startLine !== undefined || input.lineCount !== undefined
    const hasCharMode = input.offset !== undefined || input.limit !== undefined
    if (hasLineMode && hasCharMode) {
      throw new Error('startLine/lineCount and offset/limit are mutually exclusive; use one mode only')
    }
    if (hasLineMode) {
      // RD-v1: startLine/lineCount must be paired positive integers; no silent defaults.
      const rawStart = input.startLine
      const rawCount = input.lineCount
      const okInt = (v) => typeof v === 'number' && Number.isInteger(v) && v >= 1
      if (!okInt(rawStart) || !okInt(rawCount)) {
        throw new Error('startLine and lineCount must both be positive integers (startLine is 1-based)')
      }
      const startLine = rawStart
      const lineCount = rawCount
      const lines = contents.split(/(?<=\n)/)
      const totalLines = lines.length
      if (startLine > totalLines) {
        return textResult('', {
          path: input.path, truncated: false,
          startLine, lineCount, totalLines,
          pageComplete: true, scanComplete: true, readMode: 'lines',
        }, limits.maxOutputChars)
      }
      const endLine = Math.min(startLine - 1 + lineCount, totalLines)
      const selected = lines.slice(startLine - 1, endLine).join('')
      // RD-v1: pageComplete must reflect the actually returned range.
      // Any internal (readChars) or output (textResult) truncation means the
      // requested page was not fully returned.
      const selectedStart = lines.slice(0, startLine - 1).join('').length
      const budget = Math.min(limits.maxReadChars, limits.maxOutputChars)
      const bounded = sliceUtf16Budget(selected, budget)
      if (selected.length > 0 && bounded.length === 0) {
        throw new Error('output budget is too small for the next complete UTF-16 character; raise maxOutputChars or use a larger range')
      }
      const truncated = bounded.length < selected.length
      const deliveredLines = (bounded.match(/\n/g) || []).length
      return textResult(bounded, {
        path: input.path, truncated,
        startLine, lineCount, totalLines,
        pageComplete: endLine === totalLines && !truncated, scanComplete: true, readMode: 'lines',
        nextOffset: truncated ? selectedStart + bounded.length : null,
        nextStartLine: truncated && bounded.endsWith('\n') ? startLine + deliveredLines : null,
        returnedUnits: bounded.length, totalUnits: contents.length,
      }, limits.maxOutputChars)
    }
    // Legacy UTF-16 code-unit mode: surrogate-pair safe, with nextOffset.
    // `nextOffset` is derived from the ACTUALLY RETURNED text (after the
    // output budget), so navigation never claims more than was delivered
    // (O-REVIEW-03 item 2).
    const offset = Math.max(0, Number(input.offset) || 0)
    const limit = Math.min(limits.maxReadChars, Math.max(1, Number(input.limit) || limits.maxReadChars))
    const totalUnits = contents.length
    const page = sliceUtf16Range(contents, offset, limit)
    const finalText = sliceUtf16Budget(page.text, limits.maxOutputChars)
    if (page.text.length > 0 && finalText.length === 0) {
      throw new Error('output budget is too small for the next complete UTF-16 character; raise maxOutputChars')
    }
    const outputCut = finalText.length < page.text.length
    const sourceExhausted = page.end >= totalUnits
    const truncated = !sourceExhausted || outputCut
    // nextOffset = null only when everything up to the source end was actually
    // delivered. An output cut mid-page leaves the undelivered middle readable
    // by continuing at start + returnedUnits (the dropped high surrogate is
    // re-read whole on the next page — no loss, no lone surrogate).
    const coveredEnd = page.start + finalText.length
    const fullyDelivered = sourceExhausted && !outputCut
    return textResult(
      finalText,
      {
        path: input.path, truncated, readMode: 'utf16',
        offset: page.start,
        nextOffset: fullyDelivered ? null : coveredEnd,
        returnedUnits: finalText.length, totalUnits,
      },
      limits.maxOutputChars,
    )
  }
  if (tool === 'ls') {
    const entries = await readdir(input.path, { withFileTypes: true })
    const visible = entries.filter((e) => !isIgnored(e.name)).slice(0, limits.maxEntries)
    const lines = visible.map((entry) => `${entry.isDirectory() ? '[dir]' : '[file]'} ${entry.name}`)
    const hidden = entries.length - visible.length
    return textResult(
      lines.join('\n'),
      { path: input.path, count: visible.length, truncated: visible.length < entries.length, hiddenIgnored: hidden },
      limits.maxOutputChars,
    )
  }
  if (tool === 'find') {
    const matcher = compilePattern(input.pattern, { caseSensitive: input.caseSensitive, regex: input.regex })
    const rawCursor = input.cursor ?? null
    const binding = searchCursorBinding(input.path, {
      tool: 'find', pattern: input.pattern, caseSensitive: input.caseSensitive,
      regex: input.regex, glob: input.glob ?? '', pageSize: limits.maxEntries, authorizationScope,
    })
    const { entries, skipped, scanComplete, lastIndex } = await walk(input.path, {
      signal, maxEntries: limits.maxEntries, glob: input.glob, cursor: rawCursor, cursorBinding: binding,
    })
    // O-REVIEW-03 item 3: the cursor must follow the last DELIVERED match,
    // never the last scanned entry — matches scanned but not delivered (match
    // cap or output budget) must stay reachable on the next page.
    let hitsThisPage = 0
    const delivered = []
    let text = ''
    let outputCut = false
    for (const entry of entries) {
      if (!matcher.test(entry.name)) continue
      hitsThisPage++
      if (delivered.length >= limits.maxMatches) continue
      const candidate = text ? `${text}\n${entry.path}` : entry.path
      if (candidate.length > limits.maxOutputChars) {
        if (delivered.length === 0) throw new Error('match line exceeds the output budget; narrow the query or raise limits')
        outputCut = true
        break
      }
      text = candidate
      delivered.push(entry)
    }
    const capReached = hitsThisPage > delivered.length
    // Next-page position: after the last delivered match when the cap or the
    // output budget stopped delivery; otherwise past everything scanned (a
    // page must always progress — no infinite loop, no silent loss).
    let nextCursor = null
    if (!scanComplete || capReached || outputCut) {
      const consumed = delivered.length > 0 ? delivered[delivered.length - 1].walkIndex : lastIndex
      if (consumed > 0) nextCursor = encodeCursor({ binding, consumed, pageSize: limits.maxEntries })
    }
    const scannedFromStart = rawCursor === null
    return textResult(
      text,
      {
        count: delivered.length, returnedCount: delivered.length,
        pageComplete: !outputCut && !capReached,
        scanComplete, scanTruncated: !scanComplete,
        matchLimitReached: capReached,
        skippedIgnored: skipped.ignored,
        totalMatches: scannedFromStart && scanComplete && !capReached && !outputCut ? hitsThisPage : null,
        outputTruncated: outputCut,
        cursorConsistency: 'live', cursorVersion: 1, cursorStalePossible: true,
        nextCursor,
      },
      limits.maxOutputChars,
    )
  }
  if (tool === 'grep') {
    const matcher = compilePattern(input.pattern, { caseSensitive: input.caseSensitive, regex: input.regex })
    const rawCursor = input.cursor ?? null
    const binding = searchCursorBinding(input.path, {
      tool: 'grep', pattern: input.pattern, caseSensitive: input.caseSensitive,
      regex: input.regex, glob: input.glob ?? '', pageSize: limits.maxEntries, authorizationScope,
    })
    // O-REVIEW-03 item 3: grep's cursor is file+line shaped, so one file with
    // more hits than the cap continues mid-file instead of losing the rest.
    let resume = { f: 0, l: 0 }
    if (rawCursor) {
      const decoded = decodeCursor(rawCursor)
      if (!decoded.ok) throw new Error(`invalid cursor: ${decoded.reason}`)
      if (decoded.binding !== binding) {
        throw new Error('cursor scope mismatch: this cursor was issued for a different scope, query or page size')
      }
      if (decoded.consumed && typeof decoded.consumed === 'object') resume = decoded.consumed
    }
    // O-REVIEW-03 item 4: the advertised glob must actually filter the walk —
    // grep previously declared `glob` but never passed it, so *.txt also
    // returned .md files.
    const { entries, skipped, scanComplete, lastIndex } = await walk(input.path, {
      signal, maxEntries: limits.maxEntries, glob: input.glob,
      cursor: rawCursor ? encodeCursor({ binding, consumed: Math.max(0, resume.f - 1), pageSize: limits.maxEntries }) : null,
      cursorBinding: binding,
    })
    const files = entries.filter((entry) => !entry.directory)
    let hitsThisPage = 0
    let text = ''
    let delivered = 0
    let outputCut = false
    let capReached = false
    let unreadable = 0
    let resumeFile = 0
    let resumeLine = 0
    let brokeMidFile = false
    for (const file of files) {
      throwIfAborted(signal)
      let contents
      try {
        contents = await readFile(file.path, 'utf8')
      } catch {
        // Read errors are counted and reported, never silently a complete scan.
        unreadable++
        continue
      }
      const startLine = file.walkIndex === resume.f ? resume.l : 0
      let stop = false
      for (const [index, line] of contents.split(/\r?\n/).entries()) {
        if (index < startLine) continue
        if (!matcher.test(line)) continue
        hitsThisPage++
        if (delivered >= limits.maxMatches) {
          // The next hit exists but was not delivered: resume exactly here so
          // nothing scanned-but-undelivered is lost.
          capReached = true
          resumeFile = file.walkIndex
          resumeLine = index
          stop = true
          brokeMidFile = true
          break
        }
        const formatted = `${file.path}:${index + 1}:${sliceUtf16Budget(line, limits.maxLineChars)}`
        const candidate = text ? `${text}\n${formatted}` : formatted
        if (candidate.length > limits.maxOutputChars) {
          if (delivered === 0) throw new Error('match line exceeds the output budget; narrow the query or raise limits')
          outputCut = true
          resumeFile = file.walkIndex
          resumeLine = index
          stop = true
          brokeMidFile = true
          break
        }
        text = candidate
        delivered++
        resumeFile = file.walkIndex
        resumeLine = index + 1
      }
      if (stop) break
    }
    const capOrCut = capReached || outputCut
    const effectiveScanComplete = scanComplete && !brokeMidFile && unreadable === 0
    let nextCursor = null
    if (!scanComplete || capOrCut) {
      const consumed = (delivered > 0 || capOrCut)
        ? { f: resumeFile, l: resumeLine }
        : { f: lastIndex + 1, l: 0 }
      if (consumed.f > 0 || consumed.l > 0) nextCursor = encodeCursor({ binding, consumed, pageSize: limits.maxEntries })
    }
    const scannedFromStart = rawCursor === null
    return textResult(
      text,
      {
        count: delivered, returnedCount: delivered,
        pageComplete: !outputCut && !capReached && unreadable === 0,
        scanComplete: effectiveScanComplete, scanTruncated: !effectiveScanComplete,
        matchLimitReached: capReached,
        skippedIgnored: skipped.ignored, skippedUnreadable: unreadable,
        totalMatches: scannedFromStart && effectiveScanComplete && !capReached && !outputCut ? hitsThisPage : null,
        outputTruncated: outputCut,
        cursorConsistency: 'live', cursorVersion: 1, cursorStalePossible: true,
        nextCursor,
      },
      limits.maxOutputChars,
    )
  }
  throw new Error(`Unknown read-only tool: ${tool}`)
}
