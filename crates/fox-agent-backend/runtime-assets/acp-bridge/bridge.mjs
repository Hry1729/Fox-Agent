// Fox ACP bridge (docs/sandbox.md 7.12). Runs INSIDE the srt sandbox on
// Windows: `node bridge.mjs -- <backend program> [args...]`.
//
// srt 0.0.80 on Windows does not deliver stdin to the sandboxed process (the
// srt-win broker closes the runner's stdin right after writing its launch
// spec), so ACP cannot run over the backend's stdin. Instead Fox listens on a
// one-shot named pipe; this script connects to it (FOX_ACP_PIPE), sends
// `FOX-ACP-BRIDGE/1 <FOX_ACP_TOKEN>\n`, waits for `OK\n`, and only then starts
// the backend and relays pipe <-> backend stdin/stdout. The backend's stderr
// goes straight to this process's stderr (srt forwards it to Fox).
//
// * Pipe closed by Fox -> backend stdin is closed; if the backend has not
//   exited after the grace period it is killed.
// * Backend exited -> the pipe is closed and this script exits with the
//   backend's exit code.
// * The token is removed from the environment before the backend starts and
//   is never written to a file or a log line.
//
// Exit codes of the bridge itself: 64 usage, 65 missing FOX_ACP_PIPE /
// FOX_ACP_TOKEN, 66 cannot connect, 67 handshake refused or timed out,
// 68 backend could not be started.
//
// Shipped byte-for-byte in the runtime package (acp-bridge\bridge.mjs); Fox
// refuses to start when the installed copy differs from the one it embeds.
import net from 'node:net'
import fs from 'node:fs'
import path from 'node:path'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'

export const PROTOCOL = 'FOX-ACP-BRIDGE/1'
export const PIPE_ENV = 'FOX_ACP_PIPE'
export const TOKEN_ENV = 'FOX_ACP_TOKEN'
export const EXIT = Object.freeze({ usage: 64, config: 65, connect: 66, handshake: 67, spawn: 68 })
export const CONNECT_TIMEOUT_MS = 30000
export const CLOSE_GRACE_MS = 5000

export class BridgeError extends Error {
  constructor(exitCode, message) {
    super(message)
    this.exitCode = exitCode
  }
}

const defaultLog = message => process.stderr.write(`fox-acp-bridge: ${message}\n`)

/** `[..., '--', program, ...args]` -> `{ program, args }`, or null. */
export function parseArgs(argv) {
  const index = argv.indexOf('--')
  if (index < 0 || index === argv.length - 1) return null
  return { program: argv[index + 1], args: argv.slice(index + 2) }
}

/** The backend's environment: everything except the bridge variables. */
export function backendEnv(env) {
  const out = { ...env }
  for (const key of Object.keys(out)) {
    const upper = key.toUpperCase()
    if (upper === PIPE_ENV || upper === TOKEN_ENV) delete out[key]
  }
  return out
}

const quoteCmd = arg => (arg === '' || /[\s&|<>^(),;=!]/.test(arg) ? `"${arg}"` : arg)

/**
 * How to start the backend. `.cmd` / `.bat` shims (pi-acp, codex-acp, dsh)
 * cannot be spawned directly on Windows; they run through cmd.exe. Fox has
 * already refused arguments containing `"`, `%` or line breaks.
 */
export function spawnSpec(program, args, platform = process.platform, env = process.env) {
  if (platform === 'win32' && /\.(cmd|bat)$/i.test(program)) {
    const comspec = env.ComSpec || env.COMSPEC || 'cmd.exe'
    const line = [program, ...args].map(quoteCmd).join(' ')
    return { file: comspec, args: ['/d', '/s', '/c', `"${line}"`], windowsVerbatimArguments: true }
  }
  return { file: program, args, windowsVerbatimArguments: false }
}

/**
 * Send the handshake and wait for the answer. Resolves with any bytes that
 * arrived after `OK\n` (Fox may already have sent its first ACP request);
 * rejects with a BridgeError (exit 67) otherwise. The socket is paused when
 * this resolves.
 */
export function handshake(socket, token, timeoutMs = CONNECT_TIMEOUT_MS) {
  return new Promise((resolve, reject) => {
    let buffered = Buffer.alloc(0)
    let settled = false
    const finish = (error, rest) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      socket.off('data', onData)
      socket.off('end', onEnd)
      socket.off('close', onEnd)
      socket.off('error', onError)
      if (error) reject(error)
      else {
        socket.pause()
        resolve(rest)
      }
    }
    const onData = chunk => {
      buffered = Buffer.concat([buffered, chunk])
      const newline = buffered.indexOf(0x0a)
      if (newline < 0) {
        if (buffered.length > 64) finish(new BridgeError(EXIT.handshake, 'handshake answer too long'))
        return
      }
      const line = buffered.subarray(0, newline).toString('utf8').replace(/\r$/, '')
      if (line === 'OK') finish(null, buffered.subarray(newline + 1))
      else finish(new BridgeError(EXIT.handshake, `handshake refused by Fox (${line === 'DENIED' ? 'DENIED' : 'unexpected answer'})`))
    }
    const onEnd = () => finish(new BridgeError(EXIT.handshake, 'Fox closed the pipe during the handshake'))
    const onError = error => finish(new BridgeError(EXIT.handshake, `pipe error during the handshake: ${error.message}`))
    const timer = setTimeout(() => finish(new BridgeError(EXIT.handshake, 'no handshake answer from Fox in time')), timeoutMs)
    socket.on('data', onData)
    socket.on('end', onEnd)
    socket.on('close', onEnd)
    socket.on('error', onError)
    socket.write(`${PROTOCOL} ${token}\n`)
  })
}

/** Connect to the pipe / socket at `address`. Rejects with exit 66. */
export function connectPipe(address, timeoutMs = CONNECT_TIMEOUT_MS) {
  return new Promise((resolve, reject) => {
    // Half-open: after Fox ends its side the backend's last output still goes out.
    const socket = net.connect({ path: address, allowHalfOpen: true })
    const timer = setTimeout(() => {
      socket.destroy()
      reject(new BridgeError(EXIT.connect, 'timed out connecting to the Fox pipe'))
    }, timeoutMs)
    socket.once('connect', () => {
      clearTimeout(timer)
      socket.off('error', onError)
      resolve(socket)
    })
    const onError = error => {
      clearTimeout(timer)
      reject(new BridgeError(EXIT.connect, `cannot connect to the Fox pipe: ${error.code || error.message}`))
    }
    socket.once('error', onError)
  })
}

/**
 * Connected + authenticated: start the backend and relay until it exits.
 * Resolves with the exit code this process should end with.
 */
export async function runBridge({
  socket,
  token,
  program,
  args,
  env = process.env,
  spawnImpl = spawn,
  platform = process.platform,
  handshakeTimeoutMs = CONNECT_TIMEOUT_MS,
  closeGraceMs = CLOSE_GRACE_MS,
  log = defaultLog,
}) {
  let rest
  try {
    rest = await handshake(socket, token, handshakeTimeoutMs)
  } catch (error) {
    socket.destroy()
    throw error
  }
  const spec = spawnSpec(program, args, platform, env)
  let child
  try {
    child = spawnImpl(spec.file, spec.args, {
      stdio: ['pipe', 'pipe', 'inherit'],
      env: backendEnv(env),
      windowsHide: true,
      windowsVerbatimArguments: spec.windowsVerbatimArguments,
    })
  } catch (error) {
    socket.destroy()
    throw new BridgeError(EXIT.spawn, `cannot start the backend: ${error.message}`)
  }
  return await new Promise(resolve => {
    let exited = false
    let killTimer = null
    let done = false
    const settle = code => {
      if (done) return
      done = true
      clearTimeout(killTimer)
      const finish = () => resolve(code)
      if (socket.destroyed) return finish()
      const fallback = setTimeout(() => {
        socket.destroy()
        finish()
      }, 1000)
      socket.once('close', () => {
        clearTimeout(fallback)
        finish()
      })
      socket.end()
    }
    child.stdin.on('error', () => {})
    socket.on('error', error => log(`pipe error: ${error.code || error.message}`))
    if (rest.length > 0) child.stdin.write(rest)
    socket.pipe(child.stdin)
    child.stdout.pipe(socket, { end: false })
    child.stdout.on('end', () => {
      if (!socket.destroyed) socket.end()
    })
    const peerGone = () => {
      if (exited || killTimer) return
      child.stdin.end()
      killTimer = setTimeout(() => {
        if (!exited) {
          log('pipe closed and the backend did not exit in time; terminating it')
          child.kill()
        }
      }, closeGraceMs)
    }
    socket.on('end', peerGone)
    socket.on('close', peerGone)
    socket.resume()
    child.on('error', error => {
      log(`backend error: ${error.message}`)
      if (!exited) {
        exited = true
        settle(EXIT.spawn)
      }
    })
    child.on('exit', () => {
      exited = true
    })
    child.on('close', (code, signal) => {
      exited = true
      settle(code ?? (signal ? 1 : 0))
    })
  })
}

async function main() {
  const parsed = parseArgs(process.argv.slice(2))
  if (!parsed) {
    defaultLog('usage: node bridge.mjs -- <program> [args...]')
    process.exit(EXIT.usage)
  }
  const address = process.env[PIPE_ENV]
  const token = process.env[TOKEN_ENV]
  // Nothing started from here on inherits them.
  delete process.env[PIPE_ENV]
  delete process.env[TOKEN_ENV]
  if (!address || !token) {
    defaultLog(`${PIPE_ENV} and ${TOKEN_ENV} must be set`)
    process.exit(EXIT.config)
  }
  try {
    const socket = await connectPipe(address)
    const code = await runBridge({ socket, token, program: parsed.program, args: parsed.args })
    process.exit(code)
  } catch (error) {
    defaultLog(error.message)
    process.exit(error instanceof BridgeError ? error.exitCode : 1)
  }
}

function isMain() {
  if (!process.argv[1]) return false
  const same = (a, b) => (process.platform === 'win32' ? a.toLowerCase() === b.toLowerCase() : a === b)
  const self = fileURLToPath(import.meta.url)
  const invoked = path.resolve(process.argv[1])
  if (same(self, invoked)) return true
  try {
    return same(fs.realpathSync(self), fs.realpathSync(invoked))
  } catch {
    return false
  }
}

if (isMain()) main()
