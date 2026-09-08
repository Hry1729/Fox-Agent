// Download only the pinned, hash-verified binary; never run upstream install hooks.
import { readFile, writeFile, mkdir, chmod, rename, rm } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const directory = fileURLToPath(new URL('../apps/desktop/src-tauri/resources/officecli/', import.meta.url))
const release = JSON.parse(await readFile(path.join(directory, 'release.json'), 'utf8'))
const platform = { win32: 'win', darwin: 'mac', linux: 'linux' }[process.platform]
const target = process.env.FOX_OFFICECLI_TARGET || `${platform}-${process.arch}`
const asset = release.assets[target]
if (!asset) throw new Error(`OfficeCLI does not support target ${target}`)
const binary = path.join(directory, target.startsWith('win-') ? 'officecli.exe' : 'officecli')
const hash = bytes => createHash('sha256').update(bytes).digest('hex')
let current
try { current = await readFile(binary) } catch (error) { if (error.code !== 'ENOENT') throw error }
if (!current || hash(current) !== asset.sha256) {
  const response = await fetch(`${release.repository}/releases/download/v${release.version}/${asset.name}`, { signal: AbortSignal.timeout(120000) })
  if (!response.ok) throw new Error(`OfficeCLI download failed: HTTP ${response.status}`)
  const bytes = Buffer.from(await response.arrayBuffer())
  if (hash(bytes) !== asset.sha256) throw new Error('OfficeCLI SHA-256 verification failed')
  await mkdir(directory, { recursive: true })
  const temporary = `${binary}.download`
  await writeFile(temporary, bytes)
  // Only replaces the single managed binary, after verification.
  await rm(binary, { force: true })
  await rename(temporary, binary)
}
if (!target.startsWith('win-')) await chmod(binary, 0o755)
console.log(`OfficeCLI ${release.version} (${target}) verified`)
