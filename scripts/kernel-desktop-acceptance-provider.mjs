// Loopback-only, deterministic OpenAI-compatible provider for the isolated
// desktop acceptance launcher. It never contacts a remote model or logs input.
import http from 'node:http'

const port = Number(process.env.FOX_ACCEPTANCE_PROVIDER_PORT || 1433)
const stats = { requests: 0, active: 0, completed: 0, disconnected: 0, tools: [] }
function content(value) {
  if (typeof value === 'string') return value
  if (Array.isArray(value)) return value.map(block => block.text || '').join('')
  return ''
}
function findChildId(value, depth = 0) {
  if (depth > 8 || value == null) return null
  if (typeof value === 'string') {
    try { return findChildId(JSON.parse(value), depth + 1) } catch { return null }
  }
  if (typeof value !== 'object') return null
  if (typeof value.childRunId === 'string') return value.childRunId
  for (const child of Object.values(value)) {
    const id = findChildId(child, depth + 1)
    if (id) return id
  }
  return null
}
function responseFor(messages) {
  const userIndex = messages.findLastIndex(message => message.role === 'user')
  const user = content(messages[userIndex]?.content)
  const results = messages.slice(userIndex + 1).filter(message => message.role === 'tool')
  if (user.includes('[KERNEL_CHILD_WORKER]')) {
    return results.length ? { text: '子任务已读取 proof.txt，独立执行完成。' }
      : { tool: 'read', input: { path: 'proof.txt' } }
  }
  if (user.includes('[KERNEL_CHILD]')) {
    if (!results.length) return { tool: 'child_run_start', input: {
      mode: 'worker', agentId: 'fox-general', objective: '[KERNEL_CHILD_WORKER] 读取 proof.txt 并报告。',
      context: '只读取验收目录，不执行写入。',
      budget: { maxDurationMs: 60000, maxTotalTokens: 8000, maxOutputTokens: 1024, maxToolCalls: 2 },
    } }
    if (results.length === 1) {
      const id = findChildId(results[0].content)
      if (!id) return { text: '验收失败：子任务结果没有有效标识。' }
      return { tool: 'child_run_collect', input: { childRunIds: [id], waitMs: 0 } }
    }
    return { text: '父任务已收齐子任务结果，子任务执行器已退出。' }
  }
  if (user.includes('[KERNEL_APPROVAL]')) {
    if (!results.length) return { tool: 'write_file', input: {
      path: 'approval-proof.txt', content: 'Kernel 桌面审批验收：只在批准后写入。\n',
    } }
    return { text: '审批步骤已返回；请以工具结果和验收文件核对是否执行。' }
  }
  if (user.includes('[KERNEL_CANCEL]')) return { text: '这是等待取消的临时流式内容。'.repeat(80), slow: true }
  // Package-2-E: the drift fixture. It writes the correct merge, then streams a
  // long final response. That stretch of wall clock is the window the driver uses
  // to change a frozen source underneath the Run, which is the only way to make
  // the checker's real identity drift fire without touching its logic.
  if (user.includes('[P2E_DRIFT]')) {
    if (!results.length) {
      return { tool: 'write_file', input: { path: 'joined.txt', content: 'alpha.txt\naa\n\nzeta.txt\nz' } }
    }
    return { text: '合并已完成。等待来源复核。'.repeat(60), slow: true }
  }
  // Package-2-E: the U1 fixture. The write is real and the receipt is real — the
  // content is simply empty, which is the one shape the checker classifies as
  // "readable but carrying no verifiable evidence". An empty `content` is a
  // legitimate request (clearing a file), so this stays a genuine managed write
  // rather than a skipped one, and its receipt hash is the empty digest.
  if (user.includes('[P2E_EMPTY]')) {
    if (!results.length) {
      return { tool: 'write_file', input: { path: 'joined.txt', content: '' } }
    }
    if (results.length === 1 && /summary\.json/.test(user)) {
      return { tool: 'write_file', input: {
        path: 'summary.json',
        content: JSON.stringify({ files: ['alpha.txt', 'zeta.txt'] }, null, 2) + '\n',
      } }
    }
    return { text: '已写出 joined.txt。' }
  }
  // Package-2-E: the merge fixture. The task text carries the marker so the same
  // provider stays replayable, and both writes are real `write_file` tool calls
  // performed by the desktop's own managed-write path.
  const mergeMatch = user.match(/\[P2E_MERGE_(OK|BAD)\]/)
  if (mergeMatch) {
    const joined = mergeMatch[1] === 'OK'
      ? 'alpha.txt\naa\n\nzeta.txt\nz'
      : 'zeta.txt\nz\n\nalpha.txt\naa'
    if (!results.length) {
      return { tool: 'write_file', input: { path: 'joined.txt', content: joined } }
    }
    if (results.length === 1) {
      return { tool: 'write_file', input: {
        path: 'summary.json',
        content: JSON.stringify({ files: ['alpha.txt', 'zeta.txt'] }, null, 2) + '\n',
      } }
    }
    return { text: '已按冻结的合并顺序写出 joined.txt，并写出 summary.json 记录文件名清单。' }
  }
  return { text: 'Kernel 桌面验收：这段内容逐步显示，只有最终响应提交后才成为正式消息。流式回复完成。' }
}

const server = http.createServer(async (req, res) => {
  if (req.method === 'GET' && req.url === '/health') {
    res.setHeader('Content-Type', 'application/json')
    res.end(JSON.stringify(stats)); return
  }
  if (req.method === 'GET' && req.url?.endsWith('/models')) {
    res.setHeader('Content-Type', 'application/json')
    res.end(JSON.stringify({ object: 'list', data: [{ id: 'fox-kernel-acceptance', object: 'model' }] })); return
  }
  if (req.method !== 'POST' || !req.url?.endsWith('/chat/completions')) {
    res.writeHead(404); res.end(); return
  }
  let bytes = 0, body = ''
  for await (const chunk of req) {
    bytes += chunk.length
    if (bytes > 2 * 1024 * 1024) { res.writeHead(413); res.end(); return }
    body += chunk
  }
  let parsed
  try { parsed = JSON.parse(body) } catch { res.writeHead(400); res.end(); return }
  if (!Array.isArray(parsed.messages)) { res.writeHead(400); res.end(); return }
  const response = responseFor(parsed.messages)
  stats.requests++; stats.active++
  if (response.tool) stats.tools.push(response.tool)
  const id = `acceptance-${stats.requests}`
  const base = { id, object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000), model: 'fox-kernel-acceptance' }
  res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache', Connection: 'keep-alive' })
  const send = (delta, finish = null) => res.write(`data: ${JSON.stringify({ ...base, choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`)
  send({ role: 'assistant', content: '' })
  let ended = false, timer
  res.once('close', () => {
    clearInterval(timer)
    stats.active--
    if (!ended) stats.disconnected++
  })
  const finish = reason => {
    send({}, reason)
    res.write(`data: ${JSON.stringify({ ...base, choices: [], usage: { prompt_tokens: 32, completion_tokens: 16, total_tokens: 48 } })}\n\n`)
    ended = true; stats.completed++
    res.end('data: [DONE]\n\n')
  }
  if (response.tool) {
    send({ tool_calls: [{ index: 0, id: `call-${id}`, type: 'function', function: { name: response.tool, arguments: JSON.stringify(response.input) } }] })
    finish('tool_calls'); return
  }
  const points = [...response.text]
  let cursor = 0
  timer = setInterval(() => {
    if (res.destroyed) { clearInterval(timer); return }
    send({ content: points.slice(cursor, cursor + 2).join('') })
    cursor += 2
    if (cursor >= points.length) { clearInterval(timer); finish('stop') }
  }, response.slow ? 250 : 200)
})
server.listen(port, '127.0.0.1', () => console.log(`Kernel acceptance provider: http://127.0.0.1:${port}/v1 (synthetic fixtures only)`))
