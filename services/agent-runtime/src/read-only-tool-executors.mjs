import { readFile, readdir } from 'node:fs/promises'

const MAX_READ_CHARS = 120_000
const MAX_MATCHES = 200

function textResult(text, details = {}) {
  return { content: [{ type: 'text', text }], details }
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

export async function executeReadOnlyTool(tool, input, { signal } = {}) {
  throwIfAborted(signal)
  if (tool === 'read') {
    const contents = await readFile(input.path, 'utf8')
    const offset = Math.max(0, Number(input.offset) || 0)
    const limit = Math.min(MAX_READ_CHARS, Math.max(1, Number(input.limit) || MAX_READ_CHARS))
    return textResult(contents.slice(offset, offset + limit), { path: input.path, truncated: offset + limit < contents.length })
  }
  if (tool === 'ls') {
    const entries = await readdir(input.path, { withFileTypes: true })
    const lines = entries.map((entry) => `${entry.isDirectory() ? '[dir]' : '[file]'} ${entry.name}`)
    return textResult(lines.join('\n'), { path: input.path, count: entries.length })
  }
  if (tool === 'find') {
    const needle = String(input.pattern || '').toLowerCase()
    const matches = (await walk(input.path, { signal }))
      .filter((entry) => entry.name.toLowerCase().includes(needle))
      .slice(0, MAX_MATCHES)
    return textResult(matches.map((entry) => entry.path).join('\n'), { count: matches.length })
  }
  if (tool === 'grep') {
    const needle = String(input.pattern || '').toLowerCase()
    const matches = []
    const files = (await walk(input.path, { signal })).filter((entry) => !entry.directory)
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
          matches.push(`${file.path}:${index + 1}:${line}`)
          if (matches.length >= MAX_MATCHES) break
        }
      }
      if (matches.length >= MAX_MATCHES) break
    }
    return textResult(matches.join('\n'), { count: matches.length })
  }
  throw new Error(`Unknown read-only tool: ${tool}`)
}
