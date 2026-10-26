import fs from 'node:fs/promises'
import { createReadStream } from 'node:fs'
import path from 'node:path'
import { createHash } from 'node:crypto'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = fileURLToPath(new URL('..', import.meta.url))
const windows = path.join(root, 'apps/desktop/src-tauri/windows')
const destination = path.join(root, 'apps/desktop/src-tauri/resources/sandbox-installer')
const hash = data => createHash('sha256').update(data).digest('hex')

export async function fileHash(file) {
  const digest = createHash('sha256')
  for await (const chunk of createReadStream(file)) digest.update(chunk)
  return digest.digest('hex')
}

export async function verifyPackage(file, release) {
  if (!/^\d+\.\d+\.\d+$/.test(release.version) || !/^[a-f0-9]{64}$/.test(release.sha256)) {
    throw new Error('Invalid sandbox release metadata')
  }
  if (await fileHash(file) !== release.sha256) throw new Error('Sandbox runtime SHA256 mismatch; rebuild or use the pinned release package')
}

export async function prepareSandboxInstaller() {
  const release = JSON.parse(await fs.readFile(path.join(windows, 'sandbox-release.json'), 'utf8'))
  const backend = path.join(root, 'crates/fox-agent-backend')
  const source = await fs.readFile(path.join(backend, 'src/launch.rs'), 'utf8')
  if (!source.includes(`RUNTIME_PACKAGE_VERSION: &str = "${release.version}"`)) throw new Error('Sandbox release version differs from Host')
  const pins = JSON.parse(await fs.readFile(path.join(backend, 'runtime-assets/model-adapters/pins.json'), 'utf8'))
  if (JSON.stringify(pins.files) !== JSON.stringify(release.adapters)) throw new Error('Sandbox release adapter pins differ from Host')
  const bridge = (await fs.readFile(path.join(backend, 'runtime-assets/acp-bridge/bridge.mjs'), 'utf8')).replace(/\r\n/g, '\n')
  if (hash(bridge) !== release.bridgeSha256) throw new Error('Sandbox release bridge differs from Host')
  const packagePath = process.env.FOX_SANDBOX_RUNTIME_PACKAGE || path.join(destination, 'runtime.zip')
  try { await fs.access(packagePath) } catch {
    throw new Error('Set FOX_SANDBOX_RUNTIME_PACKAGE to the pinned fox-sandbox-runtime Windows x64 zip before building the installer')
  }
  await verifyPackage(packagePath, release)
  await fs.mkdir(destination, { recursive: true })
  const staged = path.join(destination, 'runtime.zip')
  if (path.resolve(packagePath) !== staged) {
    await fs.copyFile(packagePath, staged)
    await verifyPackage(staged, release)
  }
  await fs.copyFile(path.join(backend, 'scripts/windows/enable-sandbox.ps1'), path.join(destination, 'enable-sandbox.ps1'))
  await fs.copyFile(path.join(windows, 'install-sandbox.ps1'), path.join(destination, 'install-sandbox.ps1'))
  await fs.writeFile(path.join(destination, 'package.nsh'), `!define FOX_SANDBOX_VERSION "${release.version}"\n!define FOX_SANDBOX_SHA256 "${release.sha256}"\n`)
  console.log(`Prepared sandbox runtime ${release.version}, SHA256 ${release.sha256}`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await prepareSandboxInstaller()
}
