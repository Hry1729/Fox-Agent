// Start the desktop app against an isolated data directory and record its PID.
//
// Usage: node launch.mjs [--binary <path>] [--reset] [--json]
import { spawn } from 'node:child_process'
import { openSync } from 'node:fs'
import { join } from 'node:path'
import {
  DATA_DIR, EVIDENCE_DIR, RUN_ROOT, ensureRunDirs, nowIso, readState, resetRunDir, resolveBinary, writeState,
} from './lib/lifecycle.mjs'

const args = process.argv.slice(2)
const flag = (name) => args.includes(name)
const value = (name) => {
  const index = args.indexOf(name)
  return index >= 0 ? args[index + 1] : undefined
}

if (flag('--reset')) resetRunDir()
ensureRunDirs()

const binary = resolveBinary(value('--binary'))
if (!binary) {
  console.error('No Fox desktop binary found. Build one first, or pass --binary <path>.')
  console.error('The acceptance run does not build or install anything by itself.')
  process.exit(2)
}

const logPath = join(EVIDENCE_DIR, `app-${Date.now()}.log`)
const log = openSync(logPath, 'a')
const child = spawn(binary, [], {
  env: { ...process.env, FOX_DATA_DIR: DATA_DIR, FOX_ACCEPTANCE_RUN: 'single-pi-20261027' },
  stdio: ['ignore', log, log],
  detached: false,
})

const state = readState()
state.runs.push({ pid: child.pid, binary, dataDir: DATA_DIR, startedAt: nowIso(), log: logPath })
writeState(state)
console.log(JSON.stringify({ pid: child.pid, binary, dataDir: DATA_DIR, log: logPath, runRoot: RUN_ROOT }, null, 2))
child.unref()
