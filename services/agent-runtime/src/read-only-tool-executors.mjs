import { readFile, readdir } from 'node:fs/promises'

const DEFAULT_LIMITS = Object.freeze({
  maxReadChars: 120_000,
  maxMatches: 200,
  maxEntries: 4_000,
  maxOutputChars: 120_000,
  maxLineChars: 8_000,
})

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

async function walk(root, { signal, maxEntries = 4_000 } = {}) {
  const found = []
  const queue = [root]
  while (queue.length > 0 && found.length < maxEntries) {
    throwIfAborted(signal)
    const directory = queue.shift()
    const entries = await readdir(directory, { withFileTypes: true })
    for (const entry of entries) {
      const path = `${directory.replace(/[\\/]$/, '')}/${entry.name}`
      found.push({ path, name: entry.name, directory: entry.isDirectory() })
      if (entry.isDirectory()) queue.push(path)
      if (found.length >= maxEntries) break
    }
  }
  return found
}

export async function executeReadOnlyTool(tool, input, { signal, limits: requestedLimits } = {}) {
  const limits = normalizedLimits(requestedLimits)
  throwIfAborted(signal)
  if (tool === 'read') {
    const contents = await readFile(input.path, 'utf8')
    const offset = Math.max(0, Number(input.offset) || 0)
    const limit = Math.min(limits.maxReadChars, Math.max(1, Number(input.limit) || limits.maxReadChars))
    return textResult(
      contents.slice(offset, offset + limit),
      { path: input.path, truncated: offset + limit < contents.length },
      limits.maxOutputChars,
    )
  }
  if (tool === 'ls') {
    const entries = await readdir(input.path, { withFileTypes: true })
    const visible = entries.slice(0, limits.maxEntries)
    const lines = visible.map((entry) => `${entry.isDirectory() ? '[dir]' : '[file]'} ${entry.name}`)
    return textResult(
      lines.join('\n'),
      { path: input.path, count: visible.length, truncated: visible.length < entries.length },
      limits.maxOutputChars,
    )
  }
  if (tool === 'find') {
    const needle = String(input.pattern || '').toLowerCase()
    const matches = (await walk(input.path, { signal, maxEntries: limits.maxEntries }))
      .filter((entry) => entry.name.toLowerCase().includes(needle))
      .slice(0, limits.maxMatches)
    return textResult(
      matches.map((entry) => entry.path).join('\n'),
      { count: matches.length },
      limits.maxOutputChars,
    )
  }
  if (tool === 'grep') {
    const needle = String(input.pattern || '').toLowerCase()
    const matches = []
    const files = (await walk(input.path, { signal, maxEntries: limits.maxEntries }))
      .filter((entry) => !entry.directory)
    for (const file of files) {
      throwIfAborted(signal)
      let contents
      try {
        contents = await readFile(file.path, 'utf8')
      } catch {
        continue
      }
      for (const [index, line] of contents.split(/\r?\n/).entries()) {
        if (line.toLowerCase().includes(needle)) {
          matches.push(`${file.path}:${index + 1}:${line.slice(0, limits.maxLineChars)}`)
          if (matches.length >= limits.maxMatches) break
        }
      }
      if (matches.length >= limits.maxMatches) break
    }
    return textResult(matches.join('\n'), { count: matches.length }, limits.maxOutputChars)
  }
  throw new Error(`Unknown read-only tool: ${tool}`)
}
