import catalog from './connector-catalog.json'
import type { SaveMcpServerInput } from '@/features/settings/use-mcp-servers'

export interface ConnectorPreset {
  id: string
  name: string
  description: string
  category: string
  source: string
  setup: {
    url?: string
    oauth?: boolean
    registration?: string
    secretHeader?: string
    bearer?: boolean
    tokenPrefix?: string
    queryKey?: string
    guide?: string
    optionalToken?: boolean
    headers?: Record<string, string | undefined>
    customEndpoint?: boolean
    program?: string
    args?: string[]
    env?: string[]
    pending?: string
  }
}

export const connectorCatalog: readonly ConnectorPreset[] = catalog
export const connectorServerId = (item: ConnectorPreset) => 'fox-catalog-' + item.id
export const findConnectorPreset = (cardId: string) => connectorCatalog.find(item => 'mcp:' + connectorServerId(item) === cardId)

export function presetInput(item: ConnectorPreset, values: Record<string, string>, windows: boolean): SaveMcpServerInput {
  const { setup } = item
  if (setup.pending) throw new Error(setup.pending)
  const input: SaveMcpServerInput = { id: connectorServerId(item), name: item.name, transport: 'streamable_http', command: '', args: [] }
  const environment: Record<string, string> = {}
  function required(key: string) {
    const value = values[key]?.trim()
    if (!value) throw new Error('请填写 ' + key)
    return value
  }
  if (setup.program) {
    input.transport = 'stdio'
    input.command = windows && setup.program === 'npx' ? 'npx.cmd' : setup.program
    input.args = (setup.args ?? []).map(arg => {
      if (arg === '{directory}') return required('directory')
      if (arg === '{organization}') {
        const org = required('organization')
        if (!/^[a-z0-9][a-z0-9-]{0,49}$/i.test(org)) throw new Error('组织名称只能包含字母、数字和连字符')
        return org
      }
      return arg
    })
    for (const key of setup.env ?? []) environment[key] = required(key)
  } else {
    input.endpointUrl = setup.url ?? required('endpoint')
    if (item.id === 'lexiang') {
      const endpoint = new URL(input.endpointUrl)
      endpoint.searchParams.set('company_from', required('company_from'))
      input.endpointUrl = endpoint.href
    }
    if (setup.queryKey) environment['Fox-Query-Key'] = required('token')
    for (const [name, value] of Object.entries(setup.headers ?? {})) if (value) environment[name] = value
    if (setup.secretHeader && (values.token?.trim() || !setup.optionalToken)) {
      environment[setup.secretHeader] = (setup.tokenPrefix ?? (setup.bearer ? 'Bearer ' : '')) + required('token')
    }
  }
  if (Object.keys(environment).length) input.environment = environment
  else input.clearEnvironment = true
  return input
}
