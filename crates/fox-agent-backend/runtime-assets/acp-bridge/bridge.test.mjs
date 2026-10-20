// Unit tests for bridge.mjs (not shipped). Run: node --test runtime-assets/acp-bridge/
// The transport is a Unix socket (Linux/macOS) or a named pipe (Windows); the
// Host side here is a tiny net server playing Fox's handshake.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'
import crypto from 'node:crypto'
import fs from 'node:fs'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { runBridge, connectPipe, parseArgs, backendEnv, parseAnswer, spawnSpec, EXIT, PROTOCOL, BridgeError } from './bridge.mjs'

const TOKEN = crypto.randomBytes(16).toString('hex')
const address = () =>
  process.platform === 'win32'
    ? `\\\\.\\pipe\\fox-acp-test-${crypto.randomBytes(8).toString('hex')}`
    : path.join(os.tmpdir(), `fox-acp-test-${crypto.randomBytes(8).toString('hex')}.sock`)

// Backend for the relay test: echoes lines, reports whether it got the token,
// and ends on the application-level QUIT (END is written BEFORE it exits, so
// it travels while the pipe is still open; no half-close needed).
const ECHO = "let b='';process.stdin.on('data',d=>{b+=d;let i;while((i=b.indexOf('\\n'))>=0){const l=b.slice(0,i);b=b.slice(i+1);if(l==='QUIT'){process.stdout.write('END\\n',()=>process.exit(0));return}process.stdout.write('ECHO:'+l+'\\n')}});process.stdout.write('ENV:'+(process.env.FOX_ACP_TOKEN||'none')+'\\n')"

// Backend for the EOF tests: on stdin EOF it records that in a marker file
// (not sent back through the pipe: on Windows nothing can travel after the
// Host closes) and exits 0.
const eofBackend = marker => `process.stdin.resume();process.stdin.on('end',()=>{require('fs').writeFileSync(${JSON.stringify(marker)},'EOF');process.exit(0)})`
const markerPath = () => path.join(os.tmpdir(), `fox-acp-marker-${crypto.randomBytes(8).toString('hex')}`)

/** spawn() that counts how often the bridge closes the backend's stdin. */
function countingSpawn() {
  const counter = { stdinEnds: 0 }
  counter.spawnImpl = (file, args, options) => {
    const child = spawn(file, args, options)
    const end = child.stdin.end.bind(child.stdin)
    child.stdin.end = (...rest) => {
      counter.stdinEnds += 1
      return end(...rest)
    }
    return child
  }
  return counter
}

function waitFor(predicate, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    const started = Date.now()
    const tick = () => {
      if (predicate()) return resolve()
      if (Date.now() - started > timeoutMs) return reject(new Error('timed out waiting'))
      setTimeout(tick, 20)
    }
    tick()
  })
}

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
        if (!reply.startsWith('OK')) return socket.end()
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

test('handshake, relay both ways, token not passed to the backend, exit code (application-level QUIT/END)', async () => {
  let received
  let hostSocket
  const where = await host(goodToken, socket => {
    hostSocket = socket
    received = collect(socket)
    socket.write('hello\n')
  })
  const logs = []
  const socket = await connectPipe(where)
  const running = runBridge({
    socket, token: TOKEN, program: process.execPath, args: ['-e', ECHO],
    env: { ...process.env, FOX_ACP_TOKEN: TOKEN, FOX_ACP_PIPE: where }, log: m => logs.push(m),
  })
  await waitFor(() => received && /ECHO:hello/.test(received()))
  hostSocket.write('QUIT\n')
  const code = await running
  assert.equal(code, 0)
  await waitFor(() => /END/.test(received()))
  const out = received()
  assert.match(out, /ENV:none/, 'token removed from the backend environment')
  assert.match(out, /ECHO:hello/)
  assert.match(out, /END/, 'reply written before exit reaches the Host')
  assert.ok(!logs.join('\n').includes(TOKEN))
})

test('Host closes the pipe: the backend gets EOF and exits (marker file, not the pipe)', async () => {
  const marker = markerPath()
  const where = await host(goodToken, socket => setTimeout(() => socket.end(), 200))
  const socket = await connectPipe(where)
  const counter = countingSpawn()
  const code = await runBridge({
    socket, token: TOKEN, program: process.execPath, args: ['-e', eofBackend(marker)],
    spawnImpl: counter.spawnImpl, log: () => {},
  })
  assert.equal(code, 0)
  assert.equal(fs.readFileSync(marker, 'utf8'), 'EOF')
  assert.equal(counter.stdinEnds, 1, 'stdin closed exactly once')
  fs.rmSync(marker, { force: true })
})

test('pipe close without end (destroyed): the backend still gets EOF and exits', async () => {
  const marker = markerPath()
  const where = await host(goodToken, () => {})
  const socket = await connectPipe(where)
  let ends = 0
  socket.on('end', () => (ends += 1))
  const counter = countingSpawn()
  const running = runBridge({
    socket, token: TOKEN, program: process.execPath, args: ['-e', eofBackend(marker)],
    spawnImpl: counter.spawnImpl, log: () => {},
  })
  // Destroying the bridge's own end gives `close` with no `end` (what a
  // Windows named pipe looks like when Fox goes away).
  setTimeout(() => socket.destroy(), 200)
  const code = await running
  assert.equal(code, 0)
  assert.equal(ends, 0, 'no end event in this case')
  assert.equal(fs.readFileSync(marker, 'utf8'), 'EOF')
  assert.equal(counter.stdinEnds, 1)
  fs.rmSync(marker, { force: true })
})

test('pipe error then close: handled once, backend gets EOF and exits', async () => {
  const marker = markerPath()
  const where = await host(goodToken, () => {})
  const socket = await connectPipe(where)
  const counter = countingSpawn()
  const logs = []
  const running = runBridge({
    socket, token: TOKEN, program: process.execPath, args: ['-e', eofBackend(marker)],
    spawnImpl: counter.spawnImpl, log: m => logs.push(m),
  })
  setTimeout(() => socket.destroy(Object.assign(new Error('simulated'), { code: 'EPIPE' })), 200)
  const code = await running
  assert.equal(code, 0)
  assert.equal(fs.readFileSync(marker, 'utf8'), 'EOF')
  assert.equal(counter.stdinEnds, 1, 'error + close close stdin once')
  assert.match(logs.join('\n'), /pipe error: EPIPE/)
  fs.rmSync(marker, { force: true })
})

test('bytes sent right after OK reach the backend', async () => {
  let received
  let hostSocket
  const where = await host(line => (goodToken(line) === 'OK\n' ? 'OK\nearly\n' : 'DENIED\n'), socket => {
    hostSocket = socket
    received = collect(socket)
  })
  const socket = await connectPipe(where)
  const running = runBridge({ socket, token: TOKEN, program: process.execPath, args: ['-e', ECHO], env: process.env, log: () => {} })
  await waitFor(() => received && /ECHO:early/.test(received()))
  hostSocket.write('QUIT\n')
  assert.equal(await running, 0)
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

// Fox's answer carries the backend's environment and working directory
// (Windows: never on the srt cmd.exe line). Values cmd.exe cannot carry
// (quotes, %, line breaks) must arrive intact, and the backend must start in
// the given cwd (srt-win does not pass the caller's cwd into the sandbox).
test('answer env (quotes, %, line breaks) and cwd reach the backend', async () => {
  const cwd = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'fox-acp-cwd-')))
  const env = {
    CODEX_CONFIG: '{"web_search":"disabled"}',
    PERCENT: '100%PATH%',
    MULTI: 'a\r\nb\nc',
    OVERRIDDEN: 'from-fox',
  }
  const answer = line => (line === `${PROTOCOL} ${TOKEN}` ? `OK ${JSON.stringify({ cwd, env })}\n` : 'DENIED\n')
  let received
  const where = await host(answer, socket => {
    received = collect(socket)
  })
  const socket = await connectPipe(where)
  const report = "process.stdout.write(JSON.stringify({cwd:process.cwd(),env:{CODEX_CONFIG:process.env.CODEX_CONFIG,PERCENT:process.env.PERCENT,MULTI:process.env.MULTI,OVERRIDDEN:process.env.OVERRIDDEN,token:process.env.FOX_ACP_TOKEN||null}})+'\\n',()=>process.exit(0))"
  const code = await runBridge({
    socket, token: TOKEN, program: process.execPath, args: ['-e', report],
    env: { ...process.env, FOX_ACP_TOKEN: TOKEN, FOX_ACP_PIPE: where, OVERRIDDEN: 'from-bridge' },
  })
  assert.equal(code, 0)
  await waitFor(() => received && received().includes('\n'))
  const seen = JSON.parse(received().split('\n')[0])
  assert.equal(fs.realpathSync(seen.cwd), cwd, 'backend runs in the cwd from the answer')
  assert.equal(seen.env.CODEX_CONFIG, env.CODEX_CONFIG)
  assert.equal(seen.env.PERCENT, env.PERCENT)
  assert.equal(seen.env.MULTI, env.MULTI)
  assert.equal(seen.env.OVERRIDDEN, 'from-fox', "Fox's value wins over the bridge's own")
  assert.equal(seen.env.token, null, 'token never reaches the backend')
  fs.rmSync(cwd, { recursive: true, force: true })
})

test('without a cwd in the answer the backend keeps the bridge cwd; plain OK still accepted', async () => {
  let received
  const where = await host(goodToken, socket => {
    received = collect(socket)
  })
  const socket = await connectPipe(where)
  const code = await runBridge({
    socket, token: TOKEN, program: process.execPath, args: ['-e', "process.stdout.write(process.cwd()+'\\n',()=>process.exit(0))"],
    env: { ...process.env, FOX_ACP_TOKEN: TOKEN, FOX_ACP_PIPE: where },
  })
  assert.equal(code, 0)
  await waitFor(() => received && received().includes('\n'))
  assert.equal(received().split('\n')[0], process.cwd())
})

test('answer parsing and env merge', () => {
  assert.deepEqual(parseAnswer('OK'), { env: {}, cwd: undefined })
  assert.deepEqual(parseAnswer('OK {"env":{}}'), { env: {}, cwd: undefined })
  assert.deepEqual(parseAnswer('OK {"cwd":"C:\\\\w","env":{"A":"\\"%\\n"}}'), { env: { A: '"%\n' }, cwd: 'C:\\w' })
  for (const bad of ['DENIED', 'OKAY', 'OK ', 'OK {', 'OK []', 'OK {"env":[]}', 'OK {"env":{"A":1}}', 'OK {"env":{"A=B":"x"}}', 'OK {"cwd":""}', 'OK {"cwd":5}']) {
    assert.equal(parseAnswer(bad), null, bad)
  }
  // Windows: names are case-insensitive, Fox's value replaces any casing.
  assert.deepEqual(backendEnv({ Path: 'C:\\old', home: 'x', FOX_ACP_TOKEN: 't' }, { PATH: 'C:\\new' }, 'win32'), { home: 'x', PATH: 'C:\\new' })
  assert.deepEqual(backendEnv({ Path: '/old' }, { PATH: '/new' }, 'linux'), { Path: '/old', PATH: '/new' })
  // A bridge variable in the answer never reaches the backend either.
  assert.deepEqual(backendEnv({}, { FOX_ACP_PIPE: 'p', A: 'b' }, 'linux'), { A: 'b' })
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
