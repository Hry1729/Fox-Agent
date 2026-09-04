import { existsSync, readFileSync, statSync } from 'node:fs'
import { gzipSync } from 'node:zlib'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const distDir = fileURLToPath(new URL('../dist/', import.meta.url))
const indexPath = path.join(distDir, 'index.html')
const reportPath = path.join(distDir, 'bundle-stats.html')
const entryGzipBudget = 220 * 1024
const initialJavaScriptGzipBudget = 300 * 1024
const allowedPreloads = [
  /^\/assets\/react-vendor-[\w-]+\.js$/,
  /^\/assets\/icons-vendor-[\w-]+\.js$/,
]

const failures = []
if (!existsSync(indexPath)) failures.push('dist/index.html is missing; run the production build first')
if (!existsSync(reportPath) || statSync(reportPath).size === 0) failures.push('dist/bundle-stats.html is missing or empty')

if (failures.length === 0) {
  const html = readFileSync(indexPath, 'utf8')
  const entry = html.match(/<script[^>]+type="module"[^>]+src="([^"]+\.js)"/)?.[1]
  const preloads = [...html.matchAll(/<link[^>]+rel="modulepreload"[^>]+href="([^"]+\.js)"/g)].map((match) => match[1])
  const unexpectedPreloads = preloads.filter((asset) => !allowedPreloads.some((pattern) => pattern.test(asset)))

  if (!entry) failures.push('production entry script was not found in dist/index.html')
  if (unexpectedPreloads.length > 0) failures.push(`unexpected initial modulepreload assets: ${unexpectedPreloads.join(', ')}`)

  const initialAssets = entry ? [entry, ...preloads] : preloads
  const measurements = initialAssets.map((asset) => {
    const filePath = path.join(distDir, asset.replace(/^\//, ''))
    if (!existsSync(filePath)) {
      failures.push(`referenced initial asset is missing: ${asset}`)
      return { asset, raw: 0, gzip: 0 }
    }
    const source = readFileSync(filePath)
    return { asset, raw: source.byteLength, gzip: gzipSync(source).byteLength }
  })
  const entryMeasurement = measurements[0]
  const initialGzip = measurements.reduce((total, item) => total + item.gzip, 0)

  if (entryMeasurement && entryMeasurement.gzip > entryGzipBudget) {
    failures.push(`entry gzip ${entryMeasurement.gzip} bytes exceeds ${entryGzipBudget} byte budget`)
  }
  if (initialGzip > initialJavaScriptGzipBudget) {
    failures.push(`initial JavaScript gzip ${initialGzip} bytes exceeds ${initialJavaScriptGzipBudget} byte budget`)
  }

  const kb = (bytes) => `${(bytes / 1024).toFixed(1)} KiB`
  console.log(`bundle budget: entry ${kb(entryMeasurement?.gzip ?? 0)}, initial JS ${kb(initialGzip)}, preloads ${preloads.length}`)
}

if (failures.length > 0) {
  failures.forEach((failure) => console.error(`bundle budget failed: ${failure}`))
  process.exitCode = 1
}
