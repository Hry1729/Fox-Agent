// Unit tests for bridge.mjs (not shipped). Run: node --test runtime-assets/acp-bridge/
// The transport is a Unix socket (Linux/macOS) or a named pipe (Windows); the
// Host side here is a tiny net server playing Fox's handshake.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'
import crypto from 'node:crypto'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { runBridge, connectPipe, parseArgs, backendEnv, spawnSpec, EXIT, PROTOCOL, BridgeError } from './bridge.mjs'

const TOKEN = crypto.randomBytes(16).toString('hex')
const address = () =>
  process.platform === 'win32'
    ? `\\\\.\\pipe\\fox-acp-test-${crypto.randomBytes(8).toString('hex')}`
    : path.join(os.tmpdir(), `fox-acp-test-${crypto.randomBytes(8).toString('hex')}.sock`)

const ECHO = "process.stdin.on('data',d=>process.stdout.write('ECHO:'+d));process.stdin.on('end',()=>{process.stdout.write('END\\n');process.exit(0)});process.stdout.write('ENV:'+(process.env.FOX_ACP_TOKEN||'none')+'\\n')"

/** Host side: answers the handshake with `answer(line)`, then hands the socket to `then`. */
function host(answer, then) {
  return new Promise(resolve => {
    const where = address()
    const server = net.createServer(socket => {
      server.close()
      let buf = ''
      const onData = chunk => {
        buf += chunk
        const nl = buf.indexOf('\n')
        if (nl < 0) return
        socket.off('data', onData)
        const line = buf.slice(0, nl)
        const reply = answer(line)
        socket.write(reply)
        if (!reply.startsWith('OK\n')) return socket.end()
        then?.(socket, buf.slice(nl + 1))
      }
      socket.on('data', onData)
    })
    server.listen(where, () => resolve(where))
  })
}

const goodToken = line => (line === `${PROTOCOL} ${TOKEN}` ? 'OK\n' : 'DENIED\n')

function collect(socket) {
  let out = ''
  socket.on('data', d => (out += d))
  return () => out
}

test('handshake, relay both ways, token not passed to the backend, exit code', async () => {
  let received
  const where = await host(goodToken, socket => {
    received = collect(socket)
    socket.write('hello\n')
    setTimeout(() => socket.end(), 300)
  })
  const logs = []
  const socket = await connectPipe(where)
  const code = await runBridge({
    socket, token: TOKEN, program: process.execPath, args: ['-e', ECHO],
    env: { ...process.env, FOX_ACP_TOKEN: TOKEN, FOX_ACP_PIPE: where }, log: m => logs.push(m),
  })
  assert.equal(code, 0)
  const out = received()
  assert.match(out, /ENV:none/, 'token removed from the backend environment')
  assert.match(out, /ECHO:hello/)
  assert.match(out, /END/, 'pipe close -> backend stdin closed')
  assert.ok(!logs.join('\n').includes(TOKEN))
})

test('bytes sent right after OK reach the backend', async () => {
  let received
  const where = await host(line => (goodToken(line) === 'OK\n' ? 'OK\nearly\n' : 'DENIED\n'), socket => {
    received = collect(socket)
    setTimeout(() => socket.end(), 300)
  })
  const socket = await connectPipe(where)
  const code = await runBridge({ socket, token: TOKEN, program: process.execPath, args: ['-e', ECHO], env: process.env, log: () => {} })
  assert.equal(code, 0)
  assert.match(received(), /ECHO:early/)
})

test('wrong token: refused, backend never started', async () => {
  const where = await host(goodToken)
  const socket = await connectPipe(where)
  let spawned = false
  await assert.rejects(
    runBridge({ socket, token: 'f'.repeat(32), program: process.execPath, args: ['-e', ''], spawnImpl: () => { spawned = true }, log: () => {} }),
    error => error instanceof BridgeError && error.exitCode === EXIT.handshake && !error.message.includes('f'.repeat(32)),
  )
  assert.equal(spawned, false)
})

test('pipe closed: backend that ignores stdin EOF is terminated after the grace', async () => {
  const where = await host(goodToken, socket => setTimeout(() => socket.destroy(), 200))
  const socket = await connectPipe(where)
  const started = Date.now()
  const logs = []
  const code = await runBridge({
    socket, token: TOKEN, program: process.execPath, args: ['-e', 'setInterval(()=>{},1000)'],
    closeGraceMs: 300, log: m => logs.push(m),
  })
  assert.notEqual(code, 0)
  assert.ok(Date.now() - started < 5000)
  assert.match(logs.join('\n'), /terminating/)
})

test('backend exits first: pipe closed, its exit code returned', async () => {
  let ended = false
  const where = await host(goodToken, socket => socket.on('end', () => (ended = true)))
  const socket = await connectPipe(where)
  const code = await runBridge({ socket, token: TOKEN, program: process.execPath, args: ['-e', 'process.exit(7)'], log: () => {} })
  assert.equal(code, 7)
  await new Promise(r => setTimeout(r, 100))
  assert.equal(ended, true)
})

test('nobody listening: connect error is exit 66', async () => {
  await assert.rejects(connectPipe(address(), 2000), error => error.exitCode === EXIT.connect)
})

test('helpers', () => {
  assert.deepEqual(parseArgs(['--', 'a', 'b']), { program: 'a', args: ['b'] })
  assert.equal(parseArgs(['a']), null)
  assert.equal(parseArgs(['--']), null)
  const env = backendEnv({ fox_acp_token: 'x', FOX_ACP_PIPE: 'p', PATH: '/bin' })
  assert.deepEqual(env, { PATH: '/bin' })
  const cmd = spawnSpec('C:\\rt\\pi\\pi-acp.cmd', ['--x', 'a b'], 'win32', { ComSpec: 'C:\\Windows\\System32\\cmd.exe' })
  assert.equal(cmd.file, 'C:\\Windows\\System32\\cmd.exe')
  assert.deepEqual(cmd.args, ['/d', '/s', '/c', '"C:\\rt\\pi\\pi-acp.cmd --x "a b""'])
  assert.equal(cmd.windowsVerbatimArguments, true)
  assert.deepEqual(spawnSpec('/usr/bin/node', ['x'], 'linux'), { file: '/usr/bin/node', args: ['x'], windowsVerbatimArguments: false })
})

test('as a program: missing env is exit 65, missing -- is exit 64', async () => {
  const script = fileURLToPath(new URL('./bridge.mjs', import.meta.url))
  const run = (args, env) => new Promise(resolve => spawn(process.execPath, [script, ...args], { env, stdio: 'ignore' }).on('exit', resolve))
  const clean = { PATH: process.env.PATH }
  assert.equal(await run(['--', process.execPath], clean), EXIT.config)
  assert.equal(await run([], { ...clean, FOX_ACP_PIPE: 'x', FOX_ACP_TOKEN: 'y' }), EXIT.usage)
})
