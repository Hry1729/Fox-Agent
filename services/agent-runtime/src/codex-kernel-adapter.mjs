// Codex App Server adapter. Only dynamic Fox tool proposals are accepted; the
// native execution environments are absent and the original process is never resumed.
import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, isAbsolute } from 'node:path'
import { createHash } from 'node:crypto'
import { StringDecoder } from 'node:string_decoder'
import { openNativeModelProxy } from './native-model-proxy.mjs'
import { nativeModelSettings } from './native-model-settings.mjs'

export const CODEX_KERNEL_ADAPTER = 'codex-app-server/fox-kernel-worker-v1'
const fail = () => new Error('Codex Kernel adapter rejected the native protocol or configuration')

async function requireNativeCapabilities(command, directory, env, signal) {
  const out = join(directory, 'protocol')
  const child = spawn(command, ['app-server', 'generate-json-schema', '--experimental', '--out', out],
    { cwd: directory, env, stdio: 'ignore', windowsHide: true })
  const cancel = () => child.kill()
  signal?.addEventListener('abort', cancel, { once: true })
  if (signal?.aborted) cancel()
  const timer = setTimeout(cancel, 5000)
  try {
    const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve) })
    if (code !== 0 || signal?.aborted) throw fail()
    const start = JSON.parse(await readFile(join(out, 'v2', 'ThreadStartParams.json'), 'utf8'))
    const inject = JSON.parse(await readFile(join(out, 'v2', 'ThreadInjectItemsParams.json'), 'utf8'))
    if (!start.properties?.environments || !start.properties?.dynamicTools || !start.properties?.ephemeral || !inject.properties?.items) throw fail()
  } finally { clearTimeout(timer); signal?.removeEventListener('abort', cancel) }
}

export function codexHistory(messages) {
  const items = []
  for (const message of messages) {
    if (message.role === 'toolResult') {
      items.push({ type: 'function_call_output', call_id: message.toolCallId,
        output: JSON.stringify({ content: message.content, isError: message.isError }) })
      continue
    }
    const blocks = typeof message.content === 'string' ? [{ type: 'text', text: message.content }] : message.content
    if (!Array.isArray(blocks) || !['user', 'assistant'].includes(message.role)) throw fail()
    for (const block of blocks.filter(block => block.type === 'thinking')) {
      // Preserve the native reasoning item (including opaque encrypted state)
      // when it originated here. Foreign plain reasoning remains explicit data.
      if (block.foxCodexReasoning) {
        const item = block.foxCodexReasoning
        if (item.type !== 'reasoning') throw fail()
        items.push(item)
      } else if (block.thinking) {
        items.push({ type: 'message', role: 'assistant', content: [{ type: 'output_text', text: block.thinking }] })
      }
    }
    const content = []
    for (const block of blocks) {
      if (block.type === 'text') content.push({ type: message.role === 'user' ? 'input_text' : 'output_text', text: block.text })
      else if (block.type === 'image' && message.role === 'user') content.push({ type: 'input_image', image_url: `data:${block.mimeType};base64,${block.data}` })
      else if (!['thinking', 'toolCall'].includes(block.type)) throw fail()
    }
    if (content.length) items.push({ type: 'message', role: message.role, content })
    for (const block of blocks.filter(block => block.type === 'toolCall')) {
      items.push({ type: 'function_call', call_id: block.id, name: block.name, arguments: JSON.stringify(block.arguments) })
    }
  }
  return items
}

export async function runCodexKernelModel({ config, messages, apiKey, signal, onPreview }) {
  config = { ...config, modelService: nativeModelSettings(config.modelService) }
  const requestedEffort = config.modelService.reasoning === false ? 'off' : config.modelService.thinkingLevel
  if (requestedEffort !== undefined && !['off','none','minimal','low','medium','high','xhigh','max','ultra'].includes(requestedEffort)) throw fail()
  const effort = requestedEffort === 'off' ? 'none' : requestedEffort
  const native = config.nativeAdapter
  if (config.modelService.apiType !== 'openai-responses' || !native || !isAbsolute(native.command)
      || !/^sha256:[a-f0-9]{64}$/.test(native.binaryHash)) throw fail()
  const binary = await readFile(native.command)
  if (`sha256:${createHash('sha256').update(binary).digest('hex')}` !== native.binaryHash) throw fail()
  if (signal?.aborted) throw fail()
  const directory = await mkdtemp(join(tmpdir(), 'fox-codex-engine-'))
  const env = Object.fromEntries(['SystemRoot', 'WINDIR', 'PATH', 'TEMP', 'TMP'].filter(key => process.env[key]).map(key => [key, process.env[key]]))
  // This environment belongs only to the owned child; user config and sessions
  // are neither loaded nor modified. The value is an ephemeral proxy credential.
  env.CODEX_HOME = directory
  let proxy
  try {
    await requireNativeCapabilities(native.command, directory, env, signal)
    proxy = await openNativeModelProxy(config.modelService, apiKey, signal, config.proposalTools)
  } catch { await rm(directory, { recursive: true, force: true }); throw fail() }
  env.FOX_ENGINE_API_KEY = proxy.secret
  const child = spawn(native.command, ['app-server'], { cwd: directory, env, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
  const pending = new Map(); let nextId = 1; let buffer = ''; let received = 0
  let threadId; let turnId; let interrupting = false; let completed = false
  const texts = new Map(); const proposals = new Map(); let usage = {}
  let resolveTurn, rejectTurn
  const turn = new Promise((resolve, reject) => { resolveTurn = resolve; rejectTurn = reject })
  turn.catch(() => {})
  const reject = () => { const error = fail(); for (const p of pending.values()) p.reject(error); pending.clear(); rejectTurn(error) }
  const send = value => { if (child.stdin.destroyed) throw fail(); child.stdin.write(`${JSON.stringify(value)}\n`) }
  const rpc = (method, params) => new Promise((resolve, reject) => {
    const id = nextId++; pending.set(id, { resolve, reject }); send({ id, method, params })
  })
  const stop = () => { reject(); child.kill() }
  signal?.addEventListener('abort', stop, { once: true })
  const timer = setTimeout(stop, Math.min(config.modelService.requestTimeoutMs ?? 120000, 600000))
  child.once('error', reject)
  child.stdin.on('error', reject)
  const exited = new Promise(resolve => child.once('close', resolve))
  child.once('close', () => { if (!completed) reject() })
  child.stderr.resume() // diagnostics can contain provider data; never forward them
  const publish = () => onPreview?.([...texts.values()].join(''))
  const handle = message => {
    if (message.id !== undefined && !message.method) {
      const p = pending.get(message.id); if (!p) throw fail()
      pending.delete(message.id); message.error ? p.reject(fail()) : p.resolve(message.result); return
    }
    const p = message.params ?? {}
    if (p.threadId && threadId && p.threadId !== threadId) throw fail()
    if (message.method === 'turn/started') { turnId = p.turn?.id; return }
    if (p.turnId && turnId && p.turnId !== turnId) throw fail()
    if (message.method === 'item/agentMessage/delta') {
      texts.set(p.itemId, (texts.get(p.itemId) ?? '') + p.delta); publish()
    } else if (message.method === 'item/completed' && p.item?.type === 'agentMessage') {
      texts.set(p.item.id, p.item.text); publish()
    } else if (message.method === 'thread/tokenUsage/updated') {
      const u = p.tokenUsage?.total
      if (u) usage = { input: Math.max(0, u.inputTokens - u.cachedInputTokens - (u.cacheWriteInputTokens ?? 0)), output: u.outputTokens, cacheRead: u.cachedInputTokens, cacheWrite: u.cacheWriteInputTokens ?? 0, totalTokens: u.totalTokens }
    } else if (message.method === 'item/tool/call') {
      const name = p.tool; const id = p.callId
      if (!id || proposals.has(id) || !config.proposalTools.some(tool => tool.name === name)
          || !p.arguments || typeof p.arguments !== 'object' || Array.isArray(p.arguments)) throw fail()
      proposals.set(id, { type: 'toolCall', id, name, arguments: p.arguments })
      // Leave the dynamic request unanswered. Interrupt terminates all native
      // pending calls; Host will approve and execute the committed new batch.
      if (!interrupting) {
        interrupting = true
        void rpc('turn/interrupt', { threadId, turnId: p.turnId }).catch(reject)
      }
    } else if (message.id !== undefined && message.method) {
      // Native permissions, elicitation and resource execution are not supported.
      throw fail()
    } else if (message.method === 'item/started' && ['commandExecution','fileChange','mcpToolCall','webSearch','collabAgentToolCall'].includes(p.item?.type)) throw fail()
    else if (message.method === 'turn/completed') {
      if (p.turn?.status !== 'completed' && !(interrupting && proposals.size && p.turn?.status === 'interrupted')) throw fail()
      completed = true; resolveTurn()
    }
  }
  const decoder = new StringDecoder('utf8')
  child.stdout.on('data', chunk => {
    try {
      received += chunk.length; if (received > 8_388_608) throw fail()
      buffer += decoder.write(chunk); if (Buffer.byteLength(buffer) > 1_048_576) throw fail()
      let end
      while ((end = buffer.indexOf('\n')) >= 0) { const line = buffer.slice(0, end); buffer = buffer.slice(end + 1); if (line.trim()) handle(JSON.parse(line)) }
    } catch { stop() }
  })
  try {
    await rpc('initialize', { clientInfo: { name: 'fox-kernel', title: 'Fox Kernel', version: '1' }, capabilities: { experimentalApi: true } })
    send({ method: 'initialized', params: {} })
    const started = await rpc('thread/start', { model: config.modelService.modelId, modelProvider: 'fox', cwd: directory,
      ephemeral: true, environments: [], approvalPolicy: 'never', sandbox: 'read-only',
      baseInstructions: config.systemPrompt, developerInstructions: '',
      config: { model_provider: 'fox', 'model_providers.fox': { name: 'Fox frozen model', base_url: proxy.baseUrl,
          wire_api: 'responses', env_key: 'FOX_ENGINE_API_KEY', request_max_retries: 0, stream_max_retries: 0, supports_websockets: false },
        web_search: 'disabled', 'features.shell_tool': false, 'features.multi_agent': false, 'features.apps': false,
        'features.memories': false, 'features.personality': false, 'features.request_permissions': false,
        model_auto_compact_token_limit: 2147483647, model_context_window: config.modelService.contextWindow ?? 128000,
        project_doc_max_bytes: 0, 'history.persistence': 'none' },
      dynamicTools: config.proposalTools.map(tool => ({ name: tool.name, description: tool.description, inputSchema: tool.parameters })),
    })
    threadId = started.thread?.id
    if (!threadId) throw fail()
    const history = messages.slice(0, -1); const last = messages.at(-1)
    // Initial and post-tool model continuations both preserve the exact Host
    // checkpoint through the documented raw-item injection endpoint.
    if (last?.role === 'user') {
      if (history.length) await rpc('thread/inject_items', { threadId, items: codexHistory(history) })
    } else await rpc('thread/inject_items', { threadId, items: codexHistory(messages) })
    const content = last?.role === 'user' ? (typeof last.content === 'string' ? [{ type: 'text', text: last.content }] : last.content)
      : [{ type: 'text', text: 'Continue from the committed tool results above.' }]
    const input = content.map(block => block.type === 'text' ? { type: 'text', text: block.text, text_elements: [] }
      : block.type === 'image' ? { type: 'image', url: `data:${block.mimeType};base64,${block.data}` } : (() => { throw fail() })())
    await rpc('turn/start', { threadId, input, environments: [], ...(effort ? { effort } : {}) })
    await turn
    if (proxy.upstreamRequests !== 1) throw fail()
    const batch = new Map()
    for (const item of proxy.modelOutput ?? []) {
      if (item.type !== 'function_call') continue
      const args = JSON.parse(item.arguments)
      if (!item.call_id || batch.has(item.call_id) || !config.proposalTools.some(tool => tool.name === item.name)
          || !args || typeof args !== 'object' || Array.isArray(args)) throw fail()
      batch.set(item.call_id, { type: 'toolCall', id: item.call_id, name: item.name, arguments: args })
    }
    // Codex validates and requests the first dynamic call. The transport has
    // already retained the complete single model response, including calls the
    // native serial dispatcher cannot reach while its first call is unanswered.
    if (batch.size && !proposals.size) throw fail()
    for (const [id, proposal] of proposals) {
      if (JSON.stringify(batch.get(id)) !== JSON.stringify(proposal)) throw fail()
    }
    if (proxy.modelUsage) {
      const u = proxy.modelUsage
      const cached = u.input_tokens_details?.cached_tokens ?? 0
      usage = { input: u.input_tokens - cached, output: u.output_tokens,
        cacheRead: cached, cacheWrite: 0, totalTokens: u.total_tokens }
    }
    const output = [...texts.values()].filter(Boolean).map(text => ({ type: 'text', text }))
    for (const item of proxy.modelOutput ?? []) {
      if (item.type === 'reasoning') output.unshift({ type: 'thinking',
        thinking: (item.summary ?? []).map(part => part.text ?? '').join('\n'), foxCodexReasoning: item })
    }
    output.push(...batch.values())
    if (!output.length) throw fail()
    return { role: 'assistant', content: output, stopReason: proposals.size ? 'toolUse' : 'stop',
      api: 'openai-responses', provider: 'codex', model: config.modelService.modelId, usage, timestamp: Date.now() }
  } finally {
    clearTimeout(timer); signal?.removeEventListener('abort', stop)
    child.stdin.destroy(); child.kill(); await exited
    await proxy.close(); await rm(directory, { recursive: true, force: true })
  }
}
