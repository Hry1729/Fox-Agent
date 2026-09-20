import { existsSync, realpathSync } from 'node:fs'
import { copyFile, mkdir, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { homedir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, 'dist', 'fox-agent-runtime-x86_64-pc-windows-msvc.exe')
const temporaryOutput = `${output}.tmp.exe`
await mkdir(resolve(root, 'dist'), { recursive: true })
await rm(temporaryOutput, { force: true })

async function createBundlerWorkspace() {
  if (process.platform !== 'win32') return null
  const localModules = resolve(root, 'node_modules')
  const workspaceModules = resolve(root, '..', '..', 'node_modules')
  const sourceModules = existsSync(localModules) ? localModules : workspaceModules
  if (!existsSync(sourceModules)) {
    throw new Error('Pi Runtime dependencies are missing; run pnpm install in the Fox workspace.')
  }
  const workspace = resolve(root, '.sidecar-build')
  await rm(workspace, { recursive: true, force: true })
  await mkdir(resolve(workspace, 'node_modules'), { recursive: true })

  async function materialize(source, destination) {
    const info = await stat(source)
    if (info.isDirectory()) {
      await mkdir(destination, { recursive: true })
      for (const entry of await readdir(source, { withFileTypes: true })) {
        const nextSource = resolve(source, entry.name)
        const nextDestination = resolve(destination, entry.name)
        if (entry.isSymbolicLink()) {
          await materialize(realpathSync(nextSource), nextDestination)
        } else if (entry.isDirectory()) {
          await materialize(nextSource, nextDestination)
        } else if (entry.isFile()) {
          await copyFile(nextSource, nextDestination)
        }
      }
      return
    }
    await mkdir(dirname(destination), { recursive: true })
    await copyFile(source, destination)
  }

  function containingNodeModules(path) {
    let current = dirname(path)
    while (current !== dirname(current)) {
      if (basename(current) === 'node_modules') return current
      current = dirname(current)
    }
    return null
  }

  function dependencySource(dependency, parentSource) {
    const parentModules = parentSource ? containingNodeModules(parentSource) : null
    for (const modulesDirectory of [parentModules, sourceModules, workspaceModules].filter(Boolean)) {
      const candidate = resolve(modulesDirectory, dependency)
      if (existsSync(candidate)) return realpathSync(candidate)
    }
    return null
  }

  const materialized = new Map()
  const destinations = new Set()
  async function materializeDependency(dependency, parentSource = null, optional = false, parentDestination = null) {
    const source = dependencySource(dependency, parentSource)
    if (!source) {
      if (optional) return
      throw new Error(`Pi Runtime dependency ${dependency} is missing; run pnpm install in the Fox workspace.`)
    }
    const existing = materialized.get(dependency)
    if (existing === source) return
    // Pi and Harness can require different versions of the same package. Keep
    // the conflicting version beside its consumer instead of flattening it away.
    const destination = existing && parentDestination
      ? resolve(parentDestination, 'node_modules', dependency)
      : resolve(workspace, 'node_modules', dependency)
    if (destinations.has(destination)) return
    destinations.add(destination)
    if (!existing) materialized.set(dependency, source)
    await materialize(source, destination)

    const manifestPath = resolve(source, 'package.json')
    if (!existsSync(manifestPath)) return
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
    for (const child of Object.keys(manifest.dependencies ?? {})) {
      await materializeDependency(child, source, false, destination)
    }
    for (const child of Object.keys(manifest.optionalDependencies ?? {})) {
      await materializeDependency(child, source, true, destination)
    }
    // Harness exposes its public services through peer packages. pnpm resolves
    // these beside each package; the standalone staging tree must retain them.
    for (const child of Object.keys(manifest.peerDependencies ?? {})) {
      await materializeDependency(child, source, manifest.peerDependenciesMeta?.[child]?.optional === true, destination)
    }
  }

  for (const dependency of [
    '@earendil-works/pi-agent-core',
    '@earendil-works/pi-ai',
    '@earendil-works/pi-coding-agent',
    '@deepseek-ai/dsh-llm',
    '@deepseek-ai/dsh-llm-deepseek',
  ]) {
    await materializeDependency(dependency)
  }
  // The pinned Harness SDK reads its own version with createRequire at module
  // initialization. A single-file Bun executable has no adjacent package.json.
  // Replace only this metadata lookup in the disposable staging copy, retaining
  // the exact installed version and leaving installed SDK files untouched.
  const harnessRoot = resolve(workspace, 'node_modules', '@deepseek-ai/dsh-llm')
  const harnessManifest = JSON.parse(await readFile(resolve(harnessRoot, 'package.json'), 'utf8'))
  const harnessEntry = resolve(harnessRoot, 'lib', 'index.js')
  const harnessSource = await readFile(harnessEntry, 'utf8')
  const versionLookup = 'const { version } = createRequire(import.meta.url)("../package.json");'
  if (harnessManifest.version !== '0.1.2-rc.1' || harnessSource.split(versionLookup).length !== 2) {
    throw new Error('Harness package metadata layout changed; review the standalone adapter build')
  }
  await writeFile(harnessEntry, harnessSource.replace(versionLookup, `const version = ${JSON.stringify(harnessManifest.version)};`))
  // Stage all local modules so new transitive imports remain available to Bun.
  for (const file of await readdir(resolve(root, 'src'))) {
    if (!file.endsWith('.mjs') && !file.endsWith('.json')) continue
    await copyFile(resolve(root, 'src', file), resolve(workspace, file))
  }
  return workspace
}

const bunBinary = process.env.BUN_PATH
  ?? (process.platform === 'win32' && existsSync(join(homedir(), '.bun', 'bin', 'bun.exe'))
    ? join(homedir(), '.bun', 'bin', 'bun.exe')
    : 'bun')

const bundlerWorkspace = await createBundlerWorkspace()
const child = spawn(bunBinary, [
  'build',
  bundlerWorkspace ? 'pi-runtime.mjs' : 'src/pi-runtime.mjs',
  '--compile',
  '--target=bun-windows-x64',
  '--windows-hide-console',
  `--outfile=${temporaryOutput}`,
], { cwd: bundlerWorkspace ?? root, stdio: 'inherit' })

const exitCode = await new Promise((resolveExit, reject) => {
  child.once('error', reject)
  child.once('exit', (code) => resolveExit(code ?? 1))
})
if (exitCode !== 0) {
  if (bundlerWorkspace) await rm(bundlerWorkspace, { recursive: true, force: true })
  await rm(temporaryOutput, { force: true })
  process.exit(exitCode)
}

if (bundlerWorkspace) await rm(bundlerWorkspace, { recursive: true, force: true })
await rm(output, { force: true })
await rename(temporaryOutput, output)
