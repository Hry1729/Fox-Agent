// Verifies the profile/release build isolation after a `vite build`.
//   node scripts/check-build-isolation.mjs --mode=release
//   node scripts/check-build-isolation.mjs --mode=profile
//
// release: the profiling renderer, telemetry, harness AND the foxPerf entry MUST
//          all be absent (full DCE).
// profile: ALL FIVE must be independently present:
//          - telemetry module        (provisionalFrameBudgetMs)
//          - synthetic harness UI    (fox-perf-report)
//          - foxPerf entry           (foxPerf query handling)
//          - real-App controller     (fox-perf-real.json — NOT in the harness)
//          - react-dom profiling     (react-dom-profiling in react-vendor)
// Presence of only some is a failure — they are checked separately.
import { readFileSync, existsSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const mode = process.argv.includes('--mode=profile') ? 'profile' : 'release'
const distDir = fileURLToPath(new URL('../dist/', import.meta.url))
const assetsDir = path.join(distDir, 'assets')

const failures = []
const info = []

if (!existsSync(path.join(distDir, 'index.html'))) {
  console.error('isolation: dist/index.html missing; build first')
  process.exit(1)
}

const files = existsSync(assetsDir) ? readdirSync(assetsDir).filter((f) => f.endsWith('.js')) : []
const filesContaining = (needle) => {
  const hit = []
  for (const file of files) {
    if (readFileSync(path.join(assetsDir, file), 'utf8').includes(needle)) hit.push(file)
  }
  return hit
}

const telemetry = filesContaining('provisionalFrameBudgetMs')
const harness = filesContaining('fox-perf-report')
const realController = filesContaining('fox-perf-real.json')
const foxPerfEntry = filesContaining('foxPerf')
const profilingRenderer = filesContaining('react-dom-profiling')
const reactVendor = files.find((f) => f.startsWith('react-vendor-'))

const requireAbsent = (label, found) => {
  if (found.length) failures.push(`${label} leaked into release: ${found.join(', ')}`)
  else info.push(`release: ${label} absent`)
}
const requirePresent = (label, found) => {
  if (found.length) info.push(`profile: ${label} present (${found.length} file)`)
  else failures.push(`profile build is missing ${label}`)
}

if (mode === 'release') {
  requireAbsent('telemetry', telemetry)
  requireAbsent('synthetic harness', harness)
  requireAbsent('real-App controller', realController)
  requireAbsent('foxPerf entry', foxPerfEntry)
  requireAbsent('profiling renderer', profilingRenderer)
} else {
  requirePresent('telemetry module', telemetry)
  requirePresent('synthetic harness', harness)
  requirePresent('real-App controller', realController)
  requirePresent('foxPerf entry/control', foxPerfEntry)
  if (!reactVendor) {
    failures.push('profile build has no react-vendor chunk')
  } else if (!profilingRenderer.includes(reactVendor)) {
    failures.push(`profile react-vendor (${reactVendor}) does not use the profiling renderer`)
  } else {
    info.push(`profile: profiling renderer present in ${reactVendor}`)
  }
}

for (const line of info) console.log(`isolation: ${line}`)
if (failures.length) {
  for (const failure of failures) console.error(`isolation FAILED (${mode}): ${failure}`)
  process.exitCode = 1
} else {
  console.log(`isolation: ${mode} build OK`)
}
