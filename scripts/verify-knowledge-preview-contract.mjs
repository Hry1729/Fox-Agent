import { pathToFileURL } from 'node:url'

const ALLOWED_ARGUMENTS = new Set(['base-url', 'kb-id', 'file-id'])
const REQUEST_TIMEOUT_MS = 15_000

export function parseArguments(argv) {
  const values = new Map()
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--') continue
    if (argument === '--help' || argument === '-h') return { help: true }
    if (!argument.startsWith('--')) throw new Error(`未知参数：${argument}`)
    const name = argument.slice(2)
    if (!ALLOWED_ARGUMENTS.has(name)) throw new Error(`未知参数：${argument}`)
    const value = argv[index + 1]
    if (!value || value.startsWith('--')) throw new Error(`${argument} 缺少参数值`)
    values.set(name, value)
    index += 1
  }
  return {
    help: false,
    baseUrl: validateBaseUrl(requiredValue(values, 'base-url')),
    kbId: requiredValue(values, 'kb-id'),
    fileId: requiredValue(values, 'file-id'),
  }
}

export function validateBaseUrl(value) {
  let url
  try { url = new URL(value) } catch { throw new Error('base-url 不是有效 URL') }
  if (!['http:', 'https:'].includes(url.protocol)) throw new Error('base-url 只允许 http 或 https')
  if (url.username || url.password || url.search || url.hash) throw new Error('base-url 不能包含凭证、查询参数或片段')
  return url.toString().replace(/\/$/, '')
}

export function validateSourceHeaders(headers) {
  const size = headerSize(headers, 'content-length')
  const sourceRevision = requiredHeader(headers, 'x-source-revision')
  assertByteRanges(headers)
  const available = commaSeparated(headers.get('x-available-variants'))
  if (available.length && !available.includes('original')) throw new Error('服务未声明 original 原文件能力')
  return { size, sourceRevision }
}

export function validateContentRange(value, start, end, total) {
  const match = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(value ?? '')
  if (!match) throw new Error(`Content-Range 格式无效：${value ?? '缺失'}`)
  const actual = match.slice(1).map(Number)
  if (actual[0] !== start || actual[1] !== end || actual[2] !== total) {
    throw new Error(`Content-Range 不匹配：期望 bytes ${start}-${end}/${total}，实际 ${value}`)
  }
}

export async function verifyKnowledgePreviewContract(options, token) {
  if (!token?.trim()) throw new Error('环境变量 FOX_KNOWLEDGE_TOKEN 未设置')
  const headers = { authorization: `Bearer ${token.trim()}` }
  const originalUrl = endpoint(options.baseUrl, '/api/workspace/knowledge/download', {
    kb_id: options.kbId,
    file_id: options.fileId,
    variant: 'original',
  })
  const originalHead = await checkedFetch(originalUrl, { method: 'HEAD', headers })
  const original = validateSourceHeaders(originalHead.headers)
  const bytes = await verifyRange(originalUrl, headers, original.sourceRevision, original.size)
  return { sourceRevision: original.sourceRevision, originalBytes: original.size, sampledBytes: bytes.byteLength }
}

async function verifyRange(url, baseHeaders, revision, size) {
  const end = Math.min(size, 32) - 1
  if (end < 0) throw new Error('文件长度必须大于零')
  const response = await checkedFetch(url, {
    headers: { ...baseHeaders, range: `bytes=0-${end}`, 'if-range': revision },
  }, new Set([206]))
  validateContentRange(response.headers.get('content-range'), 0, end, size)
  assertByteRanges(response.headers)
  const bytes = new Uint8Array(await response.arrayBuffer())
  if (bytes.byteLength !== end + 1) throw new Error(`Range 长度不一致：期望 ${end + 1}，实际 ${bytes.byteLength}`)
  return bytes
}

async function checkedFetch(url, init, expectedStatuses = null) {
  let response
  try { response = await fetch(url, { ...init, signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) }) }
  catch (cause) { throw new Error(`请求失败 ${url}：${cause instanceof Error ? cause.message : String(cause)}`) }
  if (expectedStatuses ? !expectedStatuses.has(response.status) : !response.ok) {
    throw new Error(`${init.method ?? 'GET'} ${url} 返回 HTTP ${response.status}`)
  }
  return response
}

function endpoint(baseUrl, path, query) {
  const url = new URL(baseUrl)
  url.pathname = `${url.pathname.replace(/\/$/, '')}${path}`
  for (const [key, value] of Object.entries(query)) url.searchParams.set(key, value)
  return url
}

function requiredValue(values, name) {
  const value = values.get(name)?.trim()
  if (!value) throw new Error(`缺少 --${name}`)
  return value
}

function requiredHeader(headers, name) {
  const value = headers.get(name)?.trim()
  if (!value) throw new Error(`响应缺少 ${name}`)
  return value
}

function headerSize(headers, name) {
  const value = Number(requiredHeader(headers, name))
  if (!Number.isSafeInteger(value) || value <= 0) throw new Error(`${name} 必须是大于零的安全整数`)
  return value
}

function assertByteRanges(headers) {
  if (headers.get('accept-ranges')?.trim().toLowerCase() !== 'bytes') throw new Error('原文件未声明 Accept-Ranges: bytes')
}

function commaSeparated(value) {
  return (value ?? '').split(',').map((item) => item.trim().toLowerCase()).filter(Boolean)
}

function usage() {
  return '用法：\n  FOX_KNOWLEDGE_TOKEN=<token> pnpm knowledge:verify-preview -- --base-url http://127.0.0.1:5010 --kb-id <id> --file-id <id>\n\n验证原文件 HEAD、版本标识和严格 Range；不会请求 Office 转换。'
}

async function main() {
  try {
    const options = parseArguments(process.argv.slice(2))
    if (options.help) return console.log(usage())
    const result = await verifyKnowledgePreviewContract(options, process.env.FOX_KNOWLEDGE_TOKEN)
    console.log('知识库原文件预览契约验证通过')
    console.log(`原文件：${result.originalBytes} bytes · ${result.sourceRevision}`)
  } catch (cause) {
    console.error(cause instanceof Error ? cause.message : String(cause))
    console.error('\n' + usage())
    process.exitCode = 1
  }
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) await main()
