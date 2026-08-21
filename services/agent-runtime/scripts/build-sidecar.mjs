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

  const materialized = new Set()
  async function materializeDependency(dependency, parentSource = null, optional = false) {
    if (materialized.has(dependency)) return
    const source = dependencySource(dependency, parentSource)
    if (!source) {
      if (optional) return
      throw new Error(`Pi Runtime dependency ${dependency} is missing; run pnpm install in the Fox workspace.`)
    }
    materialized.add(dependency)
    await materialize(source, resolve(workspace, 'node_modules', dependency))

    const manifestPath = resolve(source, 'package.json')
    if (!existsSync(manifestPath)) return
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
    for (const child of Object.keys(manifest.dependencies ?? {})) {
      await materializeDependency(child, source)
    }
    for (const child of Object.keys(manifest.optionalDependencies ?? {})) {
      await materializeDependency(child, source, true)
    }
  }

  for (const dependency of [
    '@earendil-works/pi-agent-core',
    '@earendil-works/pi-ai',
    '@earendil-works/pi-coding-agent',
  ]) {
    await materializeDependency(dependency)
  }
  const runtimeSource = await readFile(resolve(root, 'src/pi-runtime.mjs'), 'utf8')
  const bundlerSource = runtimeSource
    .replace(
      "import { fauxAssistantMessage, registerFauxProvider } from '@earendil-works/pi-ai'",
      [
        "import { fauxAssistantMessage, registerFauxProvider } from './node_modules/@earendil-works/pi-ai/dist/providers/faux.js'",
        "import { registerApiProvider } from './node_modules/@earendil-works/pi-ai/dist/api-registry.js'",
        "import { streamAnthropic, streamSimpleAnthropic } from './node_modules/@earendil-works/pi-ai/dist/providers/anthropic.js'",
        "import { streamOpenAICompletions, streamSimpleOpenAICompletions } from './node_modules/@earendil-works/pi-ai/dist/providers/openai-completions.js'",
        "import { streamOpenAIResponses, streamSimpleOpenAIResponses } from './node_modules/@earendil-works/pi-ai/dist/providers/openai-responses.js'",
      ].join('\n'),
    )
    .replace(
      "const sessions = new Map()",
      [
        "registerApiProvider({ api: 'openai-completions', stream: streamOpenAICompletions, streamSimple: streamSimpleOpenAICompletions })",
        "registerApiProvider({ api: 'openai-responses', stream: streamOpenAIResponses, streamSimple: streamSimpleOpenAIResponses })",
        "registerApiProvider({ api: 'anthropic-messages', stream: streamAnthropic, streamSimple: streamSimpleAnthropic })",
        "",
        "const sessions = new Map()",
      ].join('\n'),
    )
  await writeFile(resolve(workspace, 'pi-runtime.mjs'), bundlerSource)
  for (const file of ['agent.js', 'agent-loop.js']) {
    const path = resolve(workspace, 'node_modules', '@earendil-works', 'pi-agent-core', 'dist', file)
    const source = await readFile(path, 'utf8')
    await writeFile(path, source.replaceAll('from "@earendil-works/pi-ai/base"', 'from "../../pi-ai/dist/base.js"'))
  }
  const piAiDist = resolve(workspace, 'node_modules', '@earendil-works', 'pi-ai', 'dist')
  const piAiBase = await readFile(resolve(piAiDist, 'base.js'), 'utf8')
  await writeFile(resolve(piAiDist, 'base.js'), piAiBase
    .split('\n')
    .filter((line) => !line.includes('./images') && !line.includes('./image-models'))
    .join('\n'))
  for (const provider of ['google.js', 'google-vertex.js', 'mistral.js', 'register-builtins.js']) {
    await rm(resolve(piAiDist, 'providers', provider), { force: true })
  }
  await writeFile(resolve(piAiDist, 'index.js'), [
    'import { clearApiProviders, registerApiProvider } from "./api-registry.js"',
    'import { streamAnthropic, streamSimpleAnthropic } from "./providers/anthropic.js"',
    'import { streamOpenAICompletions, streamSimpleOpenAICompletions } from "./providers/openai-completions.js"',
    'import { streamOpenAIResponses, streamSimpleOpenAIResponses } from "./providers/openai-responses.js"',
    'export * from "./base.js"',
    'export function resetApiProviders() {',
    '  clearApiProviders()',
    '  registerApiProvider({ api: "anthropic-messages", stream: streamAnthropic, streamSimple: streamSimpleAnthropic })',
    '  registerApiProvider({ api: "openai-completions", stream: streamOpenAICompletions, streamSimple: streamSimpleOpenAICompletions })',
    '  registerApiProvider({ api: "openai-responses", stream: streamOpenAIResponses, streamSimple: streamSimpleOpenAIResponses })',
    '}',
    'resetApiProviders()',
    '',
  ].join('\n'))
  for (const file of [
    'fox-planning-extension.mjs',
    'host-tools.mjs',
    'model-profile.mjs',
    'pi-event-mapper.mjs',
    'planner-runtime.mjs',
    'prompt-composer.mjs',
    'protocol.mjs',
    'read-only-tool-executors.mjs',
    'read-only-tools.mjs',
    'runtime-contract.mjs',
    'runtime-instructions.mjs',
    'runtime-session.mjs',
  ]) {
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
