import fs from 'node:fs/promises'
import http from 'node:http'
import net from 'node:net'
import crypto from 'node:crypto'
import { spawn } from 'node:child_process'
import readline from 'node:readline'
import path from 'node:path'

if (process.argv[2] === '--server') {
  const server = http.createServer((request, response) => {
    if (request.url === '/app.js') { response.setHeader('content-type', 'text/javascript'); response.end(`document.querySelector('button').onclick=()=>document.querySelector('#state').textContent='clicked';const ws=new WebSocket('ws://'+location.host+'/socket');ws.onmessage=e=>document.querySelector('#ws').textContent=e.data;`) }
    else { response.setHeader('content-type', 'text/html; charset=utf-8'); response.end('<!doctype html><html><head><title>Fox sandbox preview</title></head><body><h1>Fox 预览验证</h1><button>测试交互</button><p id="state">ready</p><p id="ws">connecting</p><script src="/app.js"></script></body></html>') }
  })
  server.on('upgrade', (request, socket) => {
    const key = crypto.createHash('sha1').update(request.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64')
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${key}\r\n\r\n`)
    const payload = Buffer.from('websocket-ready')
    socket.write(Buffer.concat([Buffer.from([0x81, payload.length]), payload]))
    socket.on('error', () => {})
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  await fs.writeFile('preview.json', JSON.stringify({ port: server.address().port, pid: process.pid }))
} else {
  const send = message => process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', ...message })}\n`)
  let sessionId = 'preview-session'
  const text = value => send({ method: 'session/update', params: { sessionId, update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: value } } } })
  async function proxyRequest(port, websocket = false) {
    const proxy = new URL(process.env.HTTP_PROXY || process.env.http_proxy)
    const authorization = 'Basic ' + Buffer.from(`${decodeURIComponent(proxy.username)}:${decodeURIComponent(proxy.password)}`).toString('base64')
    return new Promise((resolve, reject) => {
      const request = http.request({ host: proxy.hostname, port: proxy.port, agent: new http.Agent(), method: websocket ? 'CONNECT' : 'GET', path: websocket ? `127.0.0.1:${port}` : `http://127.0.0.1:${port}/`, headers: { host: `127.0.0.1:${port}`, 'proxy-authorization': authorization } })
      const timer = setTimeout(() => { request.destroy(); reject(new Error('proxy test timed out')) }, 25000)
      request.on('response', response => { let body = ''; response.on('data', part => { body += part }); response.on('end', () => { clearTimeout(timer); resolve({ status: response.statusCode, body: body.slice(0, 400) }) }) })
      request.on('connect', (response, socket, head) => {
        if (response.statusCode !== 200) { clearTimeout(timer); socket.destroy(); resolve({ status: response.statusCode }); return }
        let bytes = head
        socket.on('data', part => {
          bytes = Buffer.concat([bytes, part])
          if (bytes.includes('websocket-ready')) { clearTimeout(timer); socket.destroy(); resolve({ status: 101, websocket: true }) }
        })
        socket.on('error', reject)
        socket.write(`GET /socket HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: Zm94LXByZXZpZXctdGVzdA==\r\nSec-WebSocket-Version: 13\r\n\r\n`)
      })
      request.on('error', error => { clearTimeout(timer); reject(error) })
      request.end()
    })
  }
  readline.createInterface({ input: process.stdin }).on('line', async line => {
    const message = JSON.parse(line)
    try {
      if (message.method === 'initialize') send({ id: message.id, result: { protocolVersion: 1, agentCapabilities: { loadSession: false }, agentInfo: { name: 'network-preview-fixture', version: '1' }, authMethods: [] } })
      else if (message.method === 'session/new') send({ id: message.id, result: { sessionId } })
      else if (message.method === 'session/cancel') { /* the test cancels pending network through Host */ }
      else if (message.method === 'session/prompt') {
        const command = message.params.prompt[0].text
        if (command === 'filesystem-boundary') {
          const canary = path.join(path.dirname(process.argv[1]), '..', 'outside-canary.txt')
          const readable = await fs.readFile(canary, 'utf8').then(() => true).catch(() => false)
          const writable = await fs.writeFile(canary, 'TEST_WRITE').then(() => true).catch(() => false)
          text(JSON.stringify({ outsideReadBlocked: !readable, outsideWriteBlocked: !writable }))
        } else if (command === 'install-dependency') {
          await fs.writeFile('package.json', JSON.stringify({ name: 'fox-network-fixture', private: true, version: '1.0.0' }))
          const npm = process.env.FOX_TEST_NPM_CLI || path.join(path.dirname(process.execPath), 'node_modules/npm/bin/npm-cli.js')
          const child = spawn(process.execPath, [npm, 'install', '--prefix', process.cwd(), '--ignore-scripts', '--no-audit', '--no-fund', '--cache', '.npm-cache', 'is-number@7.0.0'], {
            windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
            env: { ...process.env, npm_config_registry: 'https://registry.npmjs.org', npm_config_proxy: process.env.HTTP_PROXY, npm_config_https_proxy: process.env.HTTPS_PROXY, NO_PROXY: '', no_proxy: '' },
          })
          let diagnostic = ''
          child.stdout.on('data', part => { diagnostic += part }); child.stderr.on('data', part => { diagnostic += part })
          const timer = setTimeout(() => child.kill(), 45000)
          const code = await new Promise((resolve, reject) => { child.on('error', reject); child.on('exit', resolve) })
          clearTimeout(timer)
          const installed = await fs.readFile('node_modules/is-number/package.json', 'utf8').then(JSON.parse).catch(() => null)
          text(JSON.stringify({ code, installed: installed?.version, diagnostic: diagnostic.slice(-1500) }))
        } else if (command === 'start-preview') {
          spawn(process.execPath, [process.argv[1], '--server'], { stdio: 'ignore', windowsHide: true })
          let preview
          for (let attempt = 0; attempt < 100; attempt++) {
            try { preview = JSON.parse(await fs.readFile('preview.json', 'utf8')); break } catch { await new Promise(resolve => setTimeout(resolve, 50)) }
          }
          text(JSON.stringify({ preview }))
        } else if (command === 'own-http' || command === 'own-websocket') {
          const preview = JSON.parse(await fs.readFile('preview.json', 'utf8'))
          text(JSON.stringify(await proxyRequest(preview.port, command === 'own-websocket')))
        } else if (command.startsWith('foreign:')) text(JSON.stringify(await proxyRequest(Number(command.slice(8)))))
        else if (command === 'direct-loopback') {
          const preview = JSON.parse(await fs.readFile('preview.json', 'utf8'))
          const direct = await new Promise(resolve => {
            const socket = net.createConnection({ host: '127.0.0.1', port: preview.port })
            const finish = connected => { socket.destroy(); resolve(connected) }
            socket.setTimeout(700, () => finish(false)); socket.on('error', () => finish(false)); socket.on('connect', () => finish(true))
          })
          text(JSON.stringify({ direct }))
        }
        send({ id: message.id, result: { stopReason: 'end_turn' } })
      }
    } catch (error) { send({ id: message.id, error: { code: -32603, message: error.message } }) }
  })
}
