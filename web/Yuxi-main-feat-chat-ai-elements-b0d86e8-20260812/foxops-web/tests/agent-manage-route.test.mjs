import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'

test('agent manage route sits above extensions in sidebar modules', async () => {
  const source = await readFile(new URL('../src/router/modules/index.ts', import.meta.url), 'utf8')
  assert.match(source, /import \{ agentManageRoutes \} from '\.\/agent-manage'/)
  assert.ok(source.indexOf('agentManageRoutes,') < source.indexOf('extensionsRoutes,'))
})

test('agent manage page uses tabs, stats and provider icons', async () => {
  const route = await readFile(
    new URL('../src/router/modules/agent-manage.ts', import.meta.url),
    'utf8'
  )
  assert.match(route, /path:\s*'\/agent\/manage'/)
  assert.match(route, /智能体管理/)
  assert.doesNotMatch(route, /children\s*:/)

  const page = await readFile(
    new URL('../src/views/agent/manage/index.vue', import.meta.url),
    'utf8'
  )
  assert.match(page, /AgentManagePanel/)
  assert.match(page, /ModelProviderManagePanel/)
  assert.match(page, /模型供应商/)
  assert.match(page, /normalizeTab/)

  const panel = await readFile(
    new URL('../src/components/agent-management/AgentManagePanel.vue', import.meta.url),
    'utf8'
  )
  assert.match(panel, /ExtensionInfoCard/)
  assert.match(panel, /FallbackAvatar/)

  const icons = await readFile(new URL('../src/utils/modelIcon.ts', import.meta.url), 'utf8')
  assert.match(icons, /getProviderIcon/)
  assert.match(icons, /lobehub\/icons-static-svg/)
})
