import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('overview route sits after extensions in sidebar modules', async () => {
  const source = await readFile(new URL('../src/router/modules/index.ts', import.meta.url), 'utf8')
  assert.match(source, /import \{ overviewRoutes \} from '\.\/overview'/)
  assert.ok(source.indexOf('extensionsRoutes,') < source.indexOf('overviewRoutes,'))
})

test('overview route is super-admin only at /overview', async () => {
  const route = await readFile(
    new URL('../src/router/modules/overview.ts', import.meta.url),
    'utf8'
  )
  assert.match(route, /path:\s*'\/overview'/)
  assert.match(route, /roles:\s*\['R_SUPER'\]/)
  assert.match(route, /数据总览/)
})

test('overview page wires StatsOverview and dashboardApi', async () => {
  const page = await readFile(
    new URL('../src/views/overview/index.vue', import.meta.url),
    'utf8'
  )
  assert.match(page, /StatsOverview/)
  assert.match(page, /dashboardApi/)
  assert.match(page, /getAllStats/)
  assert.match(page, /FeedbackDialog/)
  assert.match(page, /CallStatsCard/)
  assert.doesNotMatch(page, /page-content/)
  assert.doesNotMatch(page, /刷新数据/)
})
