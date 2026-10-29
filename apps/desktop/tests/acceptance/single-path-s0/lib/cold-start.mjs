// Cold-start measurement in its own process.
// Run: node services/agent-runtime/.probe-s0/lib/cold-start.mjs
const t0 = process.hrtime.bigint()
const rss = () => process.memoryUsage().rss / 1048576
const rssAtStart = rss()
await import('@earendil-works/pi-agent-core')
await import('@earendil-works/pi-coding-agent')
const tImported = Number(process.hrtime.bigint() - t0) / 1e6
const rssAfterImport = rss()
const { createProbeSession } = await import('./session.mjs')
const { mkdirSync } = await import('node:fs')
const cwd = '.probe-s0/tmp/p8-cold'
mkdirSync(cwd, { recursive: true })
const { Type } = await import('@earendil-works/pi-ai')
const t1 = process.hrtime.bigint()
const { session } = await createProbeSession({
  cwd, sessionDir: cwd + '/sessions',
  customTools: [{ name: 'read', label: 'read', description: 'read', parameters: Type.Object({ path: Type.String() }), async execute() { return { content: [], details: {} } } }],
})
const tSession = Number(process.hrtime.bigint() - t1) / 1e6
console.log(JSON.stringify({
  rssAtStartMiB: +rssAtStart.toFixed(1),
  importMs: +tImported.toFixed(1),
  rssAfterImportMiB: +rssAfterImport.toFixed(1),
  sessionMs: +tSession.toFixed(1),
  rssAfterSessionMiB: +rss().toFixed(1),
  rssDeltaPerSessionMiB: +(rss() - rssAfterImport).toFixed(1),
}))
void session
