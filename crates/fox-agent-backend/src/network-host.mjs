import fs from 'node:fs/promises'
import net from 'node:net'
import { spawn } from 'node:child_process'
import { pathToFileURL } from 'node:url'

const control = JSON.parse(await fs.readFile(process.argv[2], 'utf8'))
const { SandboxManager, VENDORED_SRT_WIN_EXE } = await import(pathToFileURL(control.library).href)
const socket = net.createConnection(control.address)
const pending = new Map()
let sequence = 0
let child
let authenticated = false
let buffered = ''
let stopping = false
const stop = async code => {
  if (stopping) return
  stopping = true
  for (const resolve of pending.values()) resolve(false)
  pending.clear()
  child?.kill()
  socket.destroy()
  await SandboxManager.reset().catch(() => {})
  process.exit(code)
}
const connected = new Promise((resolve, reject) => {
  socket.once('connect', () => socket.write(`FOX-ACP-BRIDGE/1 ${control.token}\n`))
  socket.on('error', reject)
  socket.on('data', chunk => {
    buffered += chunk.toString('utf8')
    if (buffered.length > 65536) { reject(new Error('Host control frame too large')); void stop(1); return }
    let index
    while ((index = buffered.indexOf('\n')) >= 0) {
      const line = buffered.slice(0, index)
      buffered = buffered.slice(index + 1)
      if (!authenticated) {
        if (!line.startsWith('OK ')) { reject(new Error('Host control handshake rejected')); void stop(1); return }
        authenticated = true
        resolve()
      } else {
        try {
          const response = JSON.parse(line)
          pending.get(response.id)?.(response.allow === true)
          pending.delete(response.id)
        } catch { void stop(1) }
      }
    }
  })
})
socket.on('close', () => { void stop(1) })
socket.on('error', () => { void stop(1) })
process.on('SIGINT', () => { void stop(130) })
process.on('SIGTERM', () => { void stop(143) })
const timer = setTimeout(() => { void stop(1) }, 30000)
try {
  await connected
  clearTimeout(timer)
  // Control credentials stay in this Host process and its private config file.
  delete control.token
  const args = control.args
  if (args[0] !== '--settings' || !args[1]) throw new Error('Missing explicit sandbox settings')
  const settings = JSON.parse(await fs.readFile(args[1], 'utf8'))
  if (process.platform === 'win32') settings.windows = { ...settings.windows, srtWin: { path: VENDORED_SRT_WIN_EXE } }
  await SandboxManager.initialize(settings, ({ host, port }) => {
    if (pending.size >= 8 || stopping) return Promise.resolve(false)
    return new Promise(resolve => {
      const id = ++sequence
      pending.set(id, resolve)
      socket.write(`${JSON.stringify({ id, host, port })}\n`)
    })
  })
  const quote = value => `'${value.replaceAll("'", "'\\''")}'`
  const command = args[2] === '-c' ? args[3] : args.slice(args[2] === '--' ? 3 : 2).map(quote).join(' ')
  if (!command) throw new Error('Missing sandbox command')
  if (process.platform === 'win32') {
    const wrapped = await SandboxManager.wrapWithSandboxArgv(command)
    child = spawn(wrapped.argv[0], wrapped.argv.slice(1), { shell: false, stdio: 'inherit', env: wrapped.env, windowsHide: true })
  } else {
    const wrapped = await SandboxManager.wrapWithSandbox(command)
    child = spawn(wrapped, { shell: true, stdio: 'inherit' })
  }
  child.on('error', () => { void stop(1) })
  child.on('exit', code => { void stop(code ?? 1) })
} catch (error) {
  console.error(`Fox sandbox network broker failed: ${error.message}`)
  await stop(1)
}
