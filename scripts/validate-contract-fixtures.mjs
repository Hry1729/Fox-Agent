import { readdir, readFile } from 'node:fs/promises'
import { dirname, isAbsolute, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url))
export const DEFAULT_MANIFEST_PATH = resolve(SCRIPT_DIRECTORY, 'contract-fixtures/manifest.json')
const MANIFEST_VERSION = 1

export async function validateContractFixtures(manifestPath = DEFAULT_MANIFEST_PATH) {
  const absoluteManifestPath = resolve(manifestPath)
  const fixtureRoot = dirname(absoluteManifestPath)
  const manifest = await readJson(absoluteManifestPath)
  const normalized = validateManifest(manifest)
  const referencedPaths = new Set([absoluteManifestPath])

  for (const fixture of normalized.fixtures) {
    const fixturePath = resolveFixturePath(fixtureRoot, fixture.path)
    assertUniquePath(referencedPaths, fixturePath, `fixture ${fixture.id}`)
    await readJson(fixturePath)
  }

  let enumMemberCount = 0
  for (const enumDefinition of normalized.enums) {
    for (const member of enumDefinition.members) {
      enumMemberCount += 1
      const fixturePath = resolveFixturePath(fixtureRoot, member.fixture)
      assertUniquePath(referencedPaths, fixturePath, `enum ${enumDefinition.name}.${member.name}`)
      const fixture = await readJson(fixturePath)
      validateEnumFixture(fixture, enumDefinition.name, member.name, fixturePath)
    }
  }

  const jsonFiles = await findJsonFiles(fixtureRoot)
  for (const jsonFile of jsonFiles) await readJson(jsonFile)
  const jsonFilePaths = new Set(jsonFiles)
  jsonFilePaths.add(absoluteManifestPath)

  return {
    manifestPath: absoluteManifestPath,
    fixtureCount: normalized.fixtures.length,
    enumCount: normalized.enums.length,
    enumMemberCount,
    jsonFileCount: jsonFilePaths.size,
  }
}

export function validateManifest(manifest) {
  if (!isRecord(manifest)) throw new Error('manifest 必须是 JSON 对象')
  if (manifest.manifestVersion !== MANIFEST_VERSION) {
    throw new Error(`manifestVersion 必须是 ${MANIFEST_VERSION}`)
  }
  if (!Array.isArray(manifest.fixtures)) throw new Error('manifest.fixtures 必须是数组')
  if (!Array.isArray(manifest.enums)) throw new Error('manifest.enums 必须是数组')

  const fixtureIds = new Set()
  const fixtures = manifest.fixtures.map((fixture, index) => {
    if (!isRecord(fixture)) throw new Error(`fixtures[${index}] 必须是对象`)
    const id = requireString(fixture.id, `fixtures[${index}].id`)
    const path = requireRelativeJsonPath(fixture.path, `fixtures[${index}].path`)
    if (fixtureIds.has(id)) throw new Error(`fixture id 重复：${id}`)
    fixtureIds.add(id)
    return { id, path }
  })

  const enumNames = new Set()
  const enums = manifest.enums.map((enumDefinition, index) => {
    if (!isRecord(enumDefinition)) throw new Error(`enums[${index}] 必须是对象`)
    const name = requireString(enumDefinition.name, `enums[${index}].name`)
    if (enumNames.has(name)) throw new Error(`enum 名称重复：${name}`)
    enumNames.add(name)
    if (!Array.isArray(enumDefinition.members) || enumDefinition.members.length === 0) {
      throw new Error(`enum ${name} 必须声明非空 members`)
    }

    const memberNames = new Set()
    const members = enumDefinition.members.map((member, memberIndex) => {
      if (!isRecord(member)) throw new Error(`enum ${name}.members[${memberIndex}] 必须是对象`)
      const memberName = requireString(member.name, `enum ${name}.members[${memberIndex}].name`)
      const fixture = requireRelativeJsonPath(member.fixture, `enum ${name}.${memberName}.fixture`)
      if (memberNames.has(memberName)) throw new Error(`enum ${name} 成员重复：${memberName}`)
      memberNames.add(memberName)
      return { name: memberName, fixture }
    })

    return { name, members }
  })

  return { manifestVersion: manifest.manifestVersion, fixtures, enums }
}

export function resolveFixturePath(fixtureRoot, fixturePath) {
  const absolutePath = resolve(fixtureRoot, fixturePath)
  const relativePath = relative(resolve(fixtureRoot), absolutePath)
  if (isAbsolute(relativePath) || relativePath === '..' || relativePath.startsWith(`..${separator()}`)) {
    throw new Error(`fixture 路径越界：${fixturePath}`)
  }
  return absolutePath
}

async function readJson(filePath) {
  let source
  try {
    source = await readFile(filePath, 'utf8')
  } catch (cause) {
    throw new Error(`无法读取 fixture：${filePath}：${cause instanceof Error ? cause.message : String(cause)}`)
  }

  try {
    return JSON.parse(source)
  } catch (cause) {
    throw new Error(`fixture JSON 无法解析：${filePath}：${cause instanceof Error ? cause.message : String(cause)}`)
  }
}

async function findJsonFiles(root) {
  const entries = await readdir(root, { withFileTypes: true })
  const files = []
  for (const entry of entries) {
    const entryPath = resolve(root, entry.name)
    if (entry.isDirectory()) files.push(...await findJsonFiles(entryPath))
    else if (entry.isFile() && entry.name.endsWith('.json') && entry.name !== 'manifest.json') files.push(entryPath)
  }
  return files
}

function validateEnumFixture(fixture, enumName, memberName, fixturePath) {
  if (!isRecord(fixture)) throw new Error(`enum fixture 必须是对象：${fixturePath}`)
  if (fixture.enum !== enumName || fixture.value !== memberName) {
    throw new Error(`enum fixture 与 manifest 不一致：${fixturePath}`)
  }
}

function assertUniquePath(paths, path, owner) {
  if (paths.has(path)) throw new Error(`fixture 路径重复：${owner} -> ${path}`)
  paths.add(path)
}

function requireString(value, field) {
  if (typeof value !== 'string' || value.trim() === '') throw new Error(`${field} 必须是非空字符串`)
  return value
}

function requireRelativeJsonPath(value, field) {
  const path = requireString(value, field).replaceAll('\\', '/')
  if (path.startsWith('/') || isAbsolute(path) || /^[A-Za-z]:\//.test(path) || path.split('/').includes('..') || !path.endsWith('.json')) {
    throw new Error(`${field} 必须是仓库内相对 JSON 路径`)
  }
  return path
}

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function separator() {
  return process.platform === 'win32' ? '\\' : '/'
}

function usage() {
  return '用法：node scripts/validate-contract-fixtures.mjs [--manifest <path>]'
}

function parseArguments(argv) {
  let manifestPath = DEFAULT_MANIFEST_PATH
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--help' || argument === '-h') return { help: true }
    if (argument !== '--manifest') throw new Error(`未知参数：${argument}`)
    const value = argv[index + 1]
    if (!value || value.startsWith('--')) throw new Error('--manifest 缺少参数值')
    manifestPath = value
    index += 1
  }
  return { help: false, manifestPath }
}

async function main() {
  try {
    const options = parseArguments(process.argv.slice(2))
    if (options.help) return console.log(usage())
    const result = await validateContractFixtures(options.manifestPath)
    console.log(`Contract fixtures valid: ${result.fixtureCount} fixtures, ${result.enumCount} enums, ${result.enumMemberCount} enum members.`)
  } catch (cause) {
    console.error(cause instanceof Error ? cause.message : String(cause))
    console.error(usage())
    process.exitCode = 1
  }
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) await main()
