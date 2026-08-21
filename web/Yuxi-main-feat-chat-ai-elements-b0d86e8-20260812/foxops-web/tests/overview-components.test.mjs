import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile, access } from 'node:fs/promises'
import { constants } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

async function assertFile(relPath) {
  await access(path.join(root, relPath), constants.F_OK)
}

test('dashboard API covers all overview endpoints', async () => {
  const source = await readFile(path.join(root, 'src/api/dashboard.ts'), 'utf8')
  for (const endpoint of [
    '/api/dashboard/stats',
    '/api/dashboard/stats/users',
    '/api/dashboard/stats/tools',
    '/api/dashboard/stats/knowledge',
    '/api/dashboard/stats/agents',
    '/api/dashboard/stats/calls/timeseries',
    '/api/dashboard/feedbacks'
  ]) {
    assert.match(source, new RegExp(endpoint.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
  }
  assert.match(source, /getAllStats/)
  assert.match(source, /Promise\.allSettled/)
})

test('overview dashboard components exist', async () => {
  const files = [
    'src/views/overview/index.vue',
    'src/components/dashboard/StatsOverview.vue',
    'src/components/dashboard/CallStatsCard.vue',
    'src/components/dashboard/UserStatsCard.vue',
    'src/components/dashboard/AgentStatsCard.vue',
    'src/components/dashboard/ToolStatsCard.vue',
    'src/components/dashboard/KnowledgeStatsCard.vue',
    'src/components/dashboard/FeedbackDialog.vue'
  ]
  for (const file of files) {
    await assertFile(file)
  }
})

test('agent distribution chart uses TOP 3 like Yuxi', async () => {
  const source = await readFile(
    path.join(root, 'src/components/dashboard/AgentStatsCard.vue'),
    'utf8'
  )
  assert.match(source, /对话\/工具调用分布 \(TOP 3\)/)
  assert.match(source, /\.slice\(0,\s*3\)/)
})
