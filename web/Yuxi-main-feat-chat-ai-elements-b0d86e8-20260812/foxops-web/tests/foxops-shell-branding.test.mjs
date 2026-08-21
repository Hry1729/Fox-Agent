import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('sidebar exports FoxOps business routes without ArtDesignPro demo centers', async () => {
  const source = await readFile(new URL('../src/router/modules/index.ts', import.meta.url), 'utf8')
  for (const route of [
    'chatRoutes',
    'workspaceRoutes',
    'agentManageRoutes',
    'extensionsRoutes',
    'overviewRoutes',
    'systemRoutes',
    'safeguardRoutes'
  ]) {
    assert.match(source, new RegExp(`\\b${route}\\b`))
  }
  for (const route of [
    'dashboardRoutes',
    'templateRoutes',
    'widgetsRoutes',
    'examplesRoutes',
    'articleRoutes',
    'resultRoutes',
    'exceptionRoutes',
    'helpRoutes'
  ]) {
    assert.doesNotMatch(source, new RegExp(`\\b${route}\\b`))
  }
})

test('top bar and settings hide unrelated ArtDesignPro features', async () => {
  const header = await readFile(
    new URL('../src/config/modules/headerBar.ts', import.meta.url),
    'utf8'
  )
  for (const feature of ['fastEnter', 'notification', 'chat', 'language']) {
    assert.match(header, new RegExp(`${feature}:[\\s\\S]*?enabled:\\s*false`))
  }

  const settings = await readFile(
    new URL('../src/components/core/layouts/art-settings-panel/index.vue', import.meta.url),
    'utf8'
  )
  assert.doesNotMatch(settings, /BoxStyleSettings|ContainerSettings|BasicSettings|SettingActions/)

  const userMenu = await readFile(
    new URL(
      '../src/components/core/layouts/art-header-bar/widget/ArtUserMenu.vue',
      import.meta.url
    ),
    'utf8'
  )
  assert.match(userMenu, /foxops-avatar\.png/)
  assert.doesNotMatch(userMenu, /toDocs|toGithub|lockScreen/)
})

test('SIPG logo switches between full and collapsed assets', async () => {
  const logo = await readFile(
    new URL('../src/components/core/base/art-logo/index.vue', import.meta.url),
    'utf8'
  )
  assert.match(logo, /@imgs\/logo\.png/)
  assert.match(logo, /@imgs\/logo-mini\.png/)
  assert.match(logo, /collapsed \? miniLogo : fullLogo/)
})
