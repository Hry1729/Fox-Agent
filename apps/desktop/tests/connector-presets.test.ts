import { expect, test } from 'bun:test'
import { connectorCatalog, findConnectorPreset, presetInput } from '../src/features/plugins/connector-presets'
import { createDefaultPluginGateway } from '../src/features/plugins/tauri-gateway'

test('browser directory paginates through the same 108 discoverable connectors', async () => {
  const gateway = createDefaultPluginGateway(false)
  const ids = new Set<string>()
  let cursor: string | undefined
  do {
    const page = await gateway.catalogList({ kind: 'mcp', pageSize: 40, cursor })
    for (const card of page.items) if (findConnectorPreset(card.id)) ids.add(card.id)
    cursor = page.nextCursor
  } while (cursor)
  expect(ids.size).toBe(108)
})

test('every catalog entry resolves to one Fox configuration or an account authorization', () => {
  expect(new Set(connectorCatalog.map(item => item.id)).size).toBe(108)
  for (const item of connectorCatalog) {
    const cardId = 'mcp:fox-catalog-' + item.id
    expect(findConnectorPreset(cardId)).toBe(item)
    expect([item.setup.url, item.setup.customEndpoint, item.setup.program].filter(Boolean)).toHaveLength(1)
    if (item.setup.oauth && !item.setup.secretHeader) continue
    const values: Record<string, string> = {
      token: 'private-fixture', endpoint: 'https://example.org/mcp', organization: 'fox-test',
      directory: 'D:/Test folder', company_from: 'test company',
    }
    for (const key of item.setup.env ?? []) values[key] = 'private-fixture'
    const input = presetInput(item, values, true)
    expect('mcp:' + input.id).toBe(cardId)
    expect(input.endpointUrl ?? input.command).toBeTruthy()
    expect(JSON.stringify([input.command, input.args, input.endpointUrl])).not.toContain('private-fixture')
    expect(input.args.some(arg => arg === '{directory}' || arg === '{organization}')).toBe(false)
  }
})

test('platform-specific credentials stay out of the endpoint and command line', () => {
  const get = (id: string) => findConnectorPreset('mcp:fox-catalog-' + id)!
  expect(presetInput(get('weiyun'), { token: 'sample' }, true).environment?.WyHeader).toBe('mcp_token=sample')
  const didi = presetInput(get('didi'), { token: 'sample' }, true)
  expect(didi.environment?.['Fox-Query-Key']).toBe('sample')
  expect(didi.endpointUrl).not.toContain('sample')
  const feishu = presetInput(get('feishu'), { token: 'sample' }, true)
  expect(feishu.environment?.['X-Lark-MCP-UAT']).toBe('sample')
  expect(feishu.environment?.['X-Lark-MCP-Allowed-Tools']).toContain('update-doc')
  expect(() => presetInput(get('azuredevops'), { organization: 'x & y' }, true)).toThrow()
})
