// Shared lifecycle helpers for the single-Pi acceptance run.
//
// Rules (from the execution plan, work package D2):
// * only processes this directory started may be stopped, always by recorded
//   PID — never by image name;
// * only the data directory this run created is used — never the operator's
//   real Fox database;
// * a missing or ambiguous fact is reported as unverified, never as a pass.
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve, sep } from 'node:path'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'

export const HERE = dirname(fileURLToPath(import.meta.url))
export const FIXTURE_DIR = resolve(HERE, '..')
export const FIXTURE = JSON.parse(readFileSync(join(FIXTURE_DIR, 'fixture.json'), 'utf8'))

/** Explicitly chosen isolated root (`FOX_ACCEPTANCE_ROOT`), else one under temp. */
const EXPLICIT_ROOT = process.env.FOX_ACCEPTANCE_ROOT ? resolve(process.env.FOX_ACCEPTANCE_ROOT) : null
/** Default isolated root, outside the product's own directories. */
export const RUN_ROOT = EXPLICIT_ROOT ?? join(tmpdir(), 'fox-acceptance-single-pi')
export const DATA_DIR = join(RUN_ROOT, 'data')
export const STATE_PATH = join(RUN_ROOT, 'state.json')
export const EVIDENCE_DIR = join(RUN_ROOT, 'evidence')

export function ensureRunDirs() {
  mkdirSync(DATA_DIR, { recursive: true })
  mkdirSync(EVIDENCE_DIR, { recursive: true })
}

export function readState() {
  if (!existsSync(STATE_PATH)) return { runs: [] }
  try {
    return JSON.parse(readFileSync(STATE_PATH, 'utf8'))
  } catch {
    return { runs: [] }
  }
}

export function writeState(state) {
  ensureRunDirs()
  writeFileSync(STATE_PATH, JSON.stringify(state, null, 2), 'utf8')
}

/** The binary under test: an explicit path, else the built desktop executable. */
export function resolveBinary(explicit) {
  const candidates = [
    explicit,
    process.env.FOX_ACCEPTANCE_BINARY,
    // FIXTURE_DIR is apps/desktop/tests/acceptance/single-pi-20261027.
    resolve(FIXTURE_DIR, '../../../src-tauri/target/release/fox-desktop.exe'),
    resolve(FIXTURE_DIR, '../../../src-tauri/target/release/Fox.exe'),
    resolve(FIXTURE_DIR, '../../../src-tauri/target/debug/fox-desktop.exe'),
    resolve(FIXTURE_DIR, '../../../src-tauri/target/debug/Fox.exe'),
  ].filter(Boolean)
  return candidates.find((candidate) => existsSync(candidate)) ?? null
}

/** Whether a PID is alive (tasklist by PID only; no image-name matching). */
export function isAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false
  const result = spawnSync('tasklist', ['/FI', `PID eq ${pid}`, '/NH', '/FO', 'CSV'], { encoding: 'utf8' })
  if (result.status !== 0) return false
  return new RegExp(`(^|\\D)${pid}(\\D|$)`).test(result.stdout) && /fox|desktop/i.test(result.stdout)
}

/** Stop one PID this run started. Graceful first; force only when asked. */
export function stopPid(pid, { force = false } = {}) {
  if (!isAlive(pid)) return { stopped: true, already: true }
  const args = force ? ['/F', '/PID', String(pid)] : ['/PID', String(pid)]
  const result = spawnSync('taskkill', args, { encoding: 'utf8' })
  return { stopped: result.status === 0, already: false, output: `${result.stdout ?? ''}${result.stderr ?? ''}`.trim() }
}

/**
 * Remove this run's isolated root.
 *
 * Refuses anything that is not (a) the default directory under the system temp
 * directory, or (b) an explicitly requested root that lies outside the product
 * checkout. This keeps a mistyped variable from deleting real data.
 */
export function resetRunDir() {
  const root = resolve(RUN_ROOT)
  const temp = resolve(tmpdir())
  const checkout = resolve(FIXTURE_DIR, '../../../../..')
  const underTemp = root.startsWith(temp + sep)
  const underCheckout = root === checkout || root.startsWith(checkout + sep)
  if (!underTemp && (!EXPLICIT_ROOT || underCheckout)) {
    throw new Error(`refusing to remove ${root}: neither under the system temp directory nor an explicit isolated root outside the checkout`)
  }
  rmSync(root, { recursive: true, force: true })
}

export function nowIso() {
  return new Date().toISOString()
}
