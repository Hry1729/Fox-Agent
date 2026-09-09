// Package Microsoft's installed, redistributable x64 CRT beside the desktop.
// No downloads, global installs, or copies from System32 are performed.
import fs from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'

if (process.platform !== 'win32') process.exit(0)
const root = fileURLToPath(new URL('..', import.meta.url))
let source = process.env.FOX_WINDOWS_CRT_DIR
if (!source) {
  const vswhere = path.join(process.env['ProgramFiles(x86)'] || 'C:/Program Files (x86)', 'Microsoft Visual Studio/Installer/vswhere.exe')
  const installation = execFileSync(vswhere, ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath'], { encoding: 'utf8', windowsHide: true }).trim()
  if (!installation) throw new Error('Install Visual Studio C++ redistributables, or set FOX_WINDOWS_CRT_DIR to the x64 Microsoft.VC143.CRT directory')
  const base = path.join(installation, 'VC/Redist/MSVC')
  const versions = (await fs.readdir(base)).filter(name => /^\d+\.\d+\.\d+$/.test(name)).sort((a, b) => b.localeCompare(a, undefined, { numeric: true }))
  if (!versions.length) throw new Error('No installed C++ redistributable version found')
  source = path.join(base, versions[0], 'x64/Microsoft.VC143.CRT')
}
source = path.resolve(source)
const names = (await fs.readdir(source)).filter(name => /^[a-z0-9_]+\.dll$/i.test(name)).sort()
for (const required of ['msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll']) {
  if (!names.includes(required)) throw new Error(`Missing required redistributable: ${required}`)
}
const files = []
for (const name of names) {
  const data = await fs.readFile(path.join(source, name))
  const pe = data.readUInt32LE(0x3c)
  if (data.toString('ascii', 0, 2) !== 'MZ' || data.readUInt32LE(pe) !== 0x4550 || data.readUInt16LE(pe + 4) !== 0x8664) throw new Error(`Redistributable is not an x64 PE file: ${name}`)
  files.push({ name, data, sha256: createHash('sha256').update(data).digest('hex') })
}
const destination = path.join(root, 'apps/desktop/src-tauri/resources/windows-runtime')
await fs.mkdir(destination, { recursive: true })
// This generated directory is exclusively owned by this preparer.
for (const name of await fs.readdir(destination)) {
  if (/\.dll$/i.test(name) && !names.includes(name)) await fs.unlink(path.join(destination, name))
}
for (const { name, data } of files) await fs.writeFile(path.join(destination, name), data)
await fs.writeFile(path.join(destination, 'manifest.json'), JSON.stringify({ architecture: 'x64', files: files.map(({ name, sha256 }) => ({ name, sha256 })) }, null, 2) + '\n')
console.log(`Prepared ${files.length} Microsoft x64 C++ redistributable DLLs`)
