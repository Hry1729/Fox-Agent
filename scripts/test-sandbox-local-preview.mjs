import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import fs from 'node:fs/promises'
import http from 'node:http'
import net from 'node:net'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const script = fileURLToPath(import.meta.url)
const delay = ms => new Promise(resolve => setTimeout(resolve, ms))
const quote = value => {
  if (/["%\r\n!]/.test(value)) throw new Error('Unsupported test path')
  return `"${value}"`
}
const readJson = async file => JSON.parse(await fs.readFile(file, 'utf8'))
const writeJson = (file, data) => fs.writeFile(file, JSON.stringify(data, null, 2))

function get(url) {
  return new Promise((resolve, reject) => {
    const request = http.get(url, { agent: new http.Agent(), timeout: 1500 }, response => {
      let body = ''
      response.on('data', part => { body += part })
      response.on('end', () => resolve({ status: response.statusCode, body }))
    })
    request.on('timeout', () => request.destroy(new Error('Host request timed out')))
    request.on('error', reject)
  })
}

const childSource = `
import http from 'node:http';
import net from 'node:net';
import fs from 'node:fs/promises';
import path from 'node:path';
const [folder, rawPort, rawOther] = process.argv.slice(2);
const port = Number(rawPort);
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const server = http.createServer((_request, response) => response.end('fox-local-preview-fixture'));
const watchdog = setTimeout(() => process.exit(2), 16000);
try {
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve); });
  await fs.writeFile(path.join(folder, 'ready.json'), JSON.stringify({ listening: true, port }));
  for (let i = 0; i < 120; i++) {
    try { await fs.access(path.join(folder, 'host-visited.json')); break; } catch {}
    await pause(100);
  }
  const direct = await new Promise(resolve => {
    const socket = net.createConnection({ host: '127.0.0.1', port });
    let settled = false;
    const finish = value => { if (settled) return; settled = true; socket.destroy(); resolve(value); };
    socket.on('connect', () => finish({ connected: true }));
    socket.on('error', error => finish({ connected: false, code: error.code }));
    socket.setTimeout(800, () => finish({ connected: false, code: 'timeout' }));
  });
  async function viaProxy(targetPort) {
    const value = process.env.HTTP_PROXY || process.env.http_proxy;
    if (!value) return { status: null, error: 'proxy_environment_missing' };
    const proxy = new URL(value);
    return await new Promise(resolve => {
      const request = http.request({ hostname: proxy.hostname, port: proxy.port, method: 'GET',
        path: 'http://127.0.0.1:' + targetPort + '/', agent: new http.Agent(), timeout: 2500,
        headers: { host: '127.0.0.1:' + targetPort,
          'proxy-authorization': 'Basic ' + Buffer.from(decodeURIComponent(proxy.username) + ':' + decodeURIComponent(proxy.password)).toString('base64') }
      }, response => { let body = ''; response.on('data', part => { body += part }); response.on('end', () => resolve({ status: response.statusCode, body: body.slice(0, 180) })); });
      request.on('timeout', () => request.destroy(new Error('proxy_timeout')));
      request.on('error', error => resolve({ status: null, error: error.code || error.message }));
      request.end();
    });
  }
  const ownPort = await viaProxy(port);
  const otherPort = await viaProxy(Number(rawOther));
  await fs.writeFile(path.join(folder, 'child-result.json'), JSON.stringify({ listening: true, port, direct, ownPort, otherPort }));
} catch (error) {
  await fs.writeFile(path.join(folder, 'child-result.json'), JSON.stringify({ error: error.code || error.message }));
  process.exitCode = 1;
} finally {
  clearTimeout(watchdog);
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
`

async function manager(folder, mode, port, otherPort, runtimeRoot) {
  const entry = path.join(runtimeRoot, 'srt/node_modules/@anthropic-ai/sandbox-runtime/dist/index.js')
  const { SandboxManager, VENDORED_SRT_WIN_EXE } = await import(pathToFileURL(entry).href)
  const node = path.join(runtimeRoot, 'node/node.exe')
  const config = {
    network: { allowedDomains: mode === 'exact-port' ? [`127.0.0.1:${port}`] : ['api.fixture.invalid'], deniedDomains: [], allowLocalBinding: false },
    filesystem: { denyRead: [], allowRead: [folder], allowWrite: [folder], denyWrite: [] },
    windows: { srtWin: { path: VENDORED_SRT_WIN_EXE } },
  }
  const decisions = []
  const decide = mode === 'host-callback' ? async request => {
    const allowed = request.host === '127.0.0.1' && request.port === Number(port)
    decisions.push({ host: request.host, port: request.port, allowed })
    return allowed
  } : undefined
  let child
  try {
    await SandboxManager.initialize(config, decide)
    const command = [node, path.join(folder, 'child.mjs'), folder, port, otherPort].map(quote).join(' ')
    const wrapped = await SandboxManager.wrapWithSandboxArgv(command, undefined, undefined, undefined, folder)
    child = spawn(wrapped.argv[0], wrapped.argv.slice(1), { shell: false, windowsHide: true, cwd: folder, env: wrapped.env, stdio: ['ignore', 'pipe', 'pipe'] })
    let stderr = ''
    child.stderr.on('data', part => { stderr += part })
    child.stdout.resume()
    const exit = new Promise((resolve, reject) => { child.on('exit', code => resolve(code)); child.on('error', reject) })
    for (let i = 0; i < 160; i++) {
      try {
        const ready = await readJson(path.join(folder, 'ready.json'))
        const visited = await get(`http://127.0.0.1:${ready.port}/`)
        await writeJson(path.join(folder, 'host-visited.json'), visited)
        break
      } catch { await delay(100) }
    }
    const code = await exit
    await writeJson(path.join(folder, 'manager-result.json'), { code, decisions, stderr: stderr.slice(-3000) })
  } finally {
    if (child?.exitCode === null) child.kill()
    await SandboxManager.reset()
  }
}

async function main() {
  assert.equal(process.platform, 'win32', 'Windows-only real runtime probe')
  assert.equal(process.env.FOX_TEST_REAL_SRT, '1', 'Set FOX_TEST_REAL_SRT=1 explicitly')
  const runtimeRoot = path.resolve(process.env.FOX_AGENT_RUNTIME_ROOT || 'C:/Program Files/Fox/sandbox-runtime/0.1.3')
  const root = path.resolve('.test-target', `local-preview-${Date.now()}`)
  await fs.mkdir(root, { recursive: true })
  const reservation = net.createServer()
  await new Promise((resolve, reject) => { reservation.once('error', reject); reservation.listen(0, '127.0.0.1', resolve) })
  const port = reservation.address().port
  await new Promise(resolve => reservation.close(resolve))
  const other = http.createServer((_request, response) => response.end('other-fixture-port'))
  await new Promise(resolve => other.listen(0, '127.0.0.1', resolve))
  const otherPort = other.address().port
  const results = []
  try {
    for (const mode of ['default-deny', 'exact-port', 'host-callback']) {
      const folder = path.join(root, mode)
      await fs.mkdir(folder)
      await fs.writeFile(path.join(folder, 'child.mjs'), childSource)
      const host = spawn(process.execPath, [script, '--manager', folder, mode, String(port), String(otherPort), runtimeRoot], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] })
      let stderr = ''
      host.stderr.on('data', part => { stderr += part })
      host.stdout.resume()
      const timer = setTimeout(() => host.kill(), 35000)
      const code = await new Promise((resolve, reject) => { host.on('exit', resolve); host.on('error', reject) })
      clearTimeout(timer)
      const result = { mode, code, stderr: stderr.slice(-3000), child: await readJson(path.join(folder, 'child-result.json')).catch(() => null), host: await readJson(path.join(folder, 'host-visited.json')).catch(() => null), manager: await readJson(path.join(folder, 'manager-result.json')).catch(() => null) }
      results.push(result)
      console.log(JSON.stringify(result))
    }
  } finally { await new Promise(resolve => other.close(resolve)) }
  await writeJson(path.join(root, 'results.json'), { runtimeRoot, srtVersion: (await readJson(path.join(runtimeRoot, 'srt/node_modules/@anthropic-ai/sandbox-runtime/package.json'))).version, results })
  console.log(`Evidence: ${path.join(root, 'results.json')}`)
  for (const result of results) {
    assert.equal(result.code, 0, result.mode)
    assert.equal(result.host?.status, 200, `${result.mode}: Host can visit the sandbox server`)
    assert.equal(result.child?.direct.connected, false, `${result.mode}: direct loopback stays fenced`)
    assert.equal(result.child?.ownPort.status, result.mode === 'default-deny' ? 403 : 200, `${result.mode}: proxy decision`)
    assert.equal(result.child?.otherPort.status, 403, `${result.mode}: other local ports stay denied`)
  }
}

if (process.argv[2] === '--manager') await manager(...process.argv.slice(3))
else await main()
