import { readFile, readdir } from 'node:fs/promises'

const DEFAULT_LIMITS = Object.freeze({
  maxReadChars: 120_000,
  maxMatches: 200,
  maxEntries: 4_000,
  maxOutputChars: 120_000,
  maxLineChars: 8_000,
})

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
    const flags = caseSensitive ? '' : 'i'
    return { test: (value) => new RegExp(pattern, flags).test(value) }
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
  const bounded = String(text || '').slice(0, maxOutputChars)
  return {
    content: [{ type: 'text', text: bounded }],
    details: { ...details, outputTruncated: bounded.length < String(text || '').length },
  }
}

function throwIfAborted(signal) {
  if (signal?.aborted) throw new Error('Tool execution was cancelled.')
}

async function walk(root, { signal, maxEntries = 4_000, glob = null } = {}) {
  const globMatcher = compileGlob(glob)
  const found = []
  const skipped = { ignored: 0 }
  const queue = [root]
  while (queue.length > 0 && found.length < maxEntries) {
    throwIfAborted(signal)
    const directory = queue.shift()
    const entries = await readdir(directory, { withFileTypes: true })
    for (const entry of entries) {
      if (isIgnored(entry.name)) { skipped.ignored++; continue }
      const path = `${directory.replace(/[\\\\/]$/, '')}/${entry.name}`
      if (globMatcher && !entry.isDirectory() && !globMatcher.test(entry.name)) continue
      found.push({ path, name: entry.name, directory: entry.isDirectory() })
      if (entry.isDirectory()) queue.push(path)
      if (found.length >= maxEntries) break
    }
  }
  return { entries: found, skipped, scanComplete: queue.length === 0 && found.length < maxEntries }
}

export async function executeReadOnlyTool(tool, input, { signal, limits: requestedLimits } = {}) {
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
      const readCharsCut = selected.length > limits.maxReadChars
      const truncated = readCharsCut || selected.length > limits.maxOutputChars
      const bounded = readCharsCut ? selected.slice(0, limits.maxReadChars) : selected
      return textResult(bounded, {
        path: input.path, truncated,
        startLine, lineCount, totalLines,
        pageComplete: endLine === totalLines && !truncated, scanComplete: true, readMode: 'lines',
      }, limits.maxOutputChars)
    }
    // Legacy UTF-16 code-unit mode (unchanged).
    const offset = Math.max(0, Number(input.offset) || 0)
    const limit = Math.min(limits.maxReadChars, Math.max(1, Number(input.limit) || limits.maxReadChars))
    return textResult(
      contents.slice(offset, offset + limit),
      { path: input.path, truncated: offset + limit < contents.length, readMode: 'utf16' },
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
    const { entries, skipped, scanComplete } = await walk(input.path, { signal, maxEntries: limits.maxEntries, glob: input.glob })
    const allMatches = entries.filter((entry) => matcher.test(entry.name))
    const matches = allMatches.slice(0, limits.maxMatches)
    const matchLimitReached = allMatches.length > limits.maxMatches
    return textResult(
      matches.map((entry) => entry.path).join('\n'),
      {
        count: matches.length,
        pageComplete: true,
        scanComplete,
        matchLimitReached,
        skippedIgnored: skipped.ignored,
        totalMatches: scanComplete && !matchLimitReached ? allMatches.length : null,
      },
      limits.maxOutputChars,
    )
  }
  if (tool === 'grep') {
    const matcher = compilePattern(input.pattern, { caseSensitive: input.caseSensitive, regex: input.regex })
    const { entries, skipped, scanComplete } = await walk(input.path, { signal, maxEntries: limits.maxEntries })
    const files = entries.filter((entry) => !entry.directory)
    const matches = []
    let totalMatchesFound = 0
    for (const file of files) {
      throwIfAborted(signal)
      let contents
      try {
        contents = await readFile(file.path, 'utf8')
      } catch {
        continue
      }
      for (const [index, line] of contents.split(/\r?\n/).entries()) {
        if (matcher.test(line)) {
          totalMatchesFound++
          if (matches.length < limits.maxMatches) {
            matches.push(`${file.path}:${index + 1}:${line.slice(0, limits.maxLineChars)}`)
          }
        }
      }
    }
    const matchLimitReached = totalMatchesFound > limits.maxMatches
    return textResult(
      matches.join('\n'),
      {
        count: matches.length,
        pageComplete: true,
        scanComplete,
        matchLimitReached,
        skippedIgnored: skipped.ignored,
        totalMatches: scanComplete && !matchLimitReached ? totalMatchesFound : null,
      },
      limits.maxOutputChars,
    )
  }
  throw new Error(`Unknown read-only tool: ${tool}`)
}
