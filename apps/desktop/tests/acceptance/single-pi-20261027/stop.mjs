// Stop the process this run started.
//
// Usage: node stop.mjs [--pid <pid>] [--force] [--json]
//   --pid    stop one recorded PID (default: the most recent one)
//   --force  force-terminate that exact PID (simulates a crash, not a close)
import { isAlive, readState, stopPid, nowIso, writeState } from './lib/lifecycle.mjs'

const args = process.argv.slice(2)
const value = (name) => {
  const index = args.indexOf(name)
  return index >= 0 ? args[index + 1] : undefined
}
const force = args.includes('--force')
const state = readState()
const explicit = Number(value('--pid'))
const target = Number.isInteger(explicit) && explicit > 0
  ? explicit
  : [...state.runs].reverse().find((run) => isAlive(run.pid))?.pid

if (!target) {
  console.error('No live process recorded by this acceptance run; nothing to stop.')
  process.exit(3)
}
if (explicit && !state.runs.some((run) => run.pid === explicit)) {
  console.error(`Refusing to stop ${explicit}: this run did not start it.`)
  process.exit(4)
}

const result = stopPid(target, { force })
state.runs = state.runs.map((run) => (run.pid === target
  ? { ...run, stoppedAt: nowIso(), stopMode: force ? 'force' : 'close', stopResult: result }
  : run))
writeState(state)
console.log(JSON.stringify({ pid: target, force, ...result }, null, 2))
process.exit(result.stopped ? 0 : 1)
