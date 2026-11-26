import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, statSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../services/agent-runtime/', import.meta.url))
const source = join(root, 'src', 'connector-auth.mjs')
const output = join(root, 'dist', 'fox-connector-auth' + (process.platform === 'win32' ? '.exe' : ''))
const lock = fileURLToPath(new URL('../pnpm-lock.yaml', import.meta.url))
if (!existsSync(output) || statSync(output).mtimeMs < Math.max(statSync(source).mtimeMs, statSync(lock).mtimeMs)) {
  mkdirSync(dirname(output), { recursive: true })
  const localBun = join(homedir(), '.bun', 'bin', process.platform === 'win32' ? 'bun.exe' : 'bun')
  const run = spawnSync(process.env.BUN_PATH ?? (existsSync(localBun) ? localBun : 'bun'),
    ['build', '--compile', ...(process.platform === 'win32' ? ['--windows-hide-console'] : []), source, '--outfile', output],
    { cwd: root, stdio: 'inherit' })
  if (run.error) throw run.error
  process.exitCode = run.status ?? 1
}
