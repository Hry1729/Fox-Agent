import { ChevronRight, Plus, Search, Server } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { servicePresets, type ServicePreset } from './service-presets'

const platformIds = new Set(['alibaba', 'openrouter', 'siliconflow-cn', 'modelscope', 'opencode'])
const isVariant = (id: string) => id.includes('-plan') || ['moonshotai', 'minimax', 'siliconflow'].includes(id)

export function ProviderCatalog({ query, onQuery, onPick }: {
  query: string
  onQuery: (value: string) => void
  onPick: (preset?: ServicePreset) => void
}) {
  const filter = query.trim().toLowerCase()
  const matching = servicePresets.filter(preset =>
    [preset.name, preset.catalog?.label ?? '', preset.id, ...preset.endpoints.map(endpoint => endpoint.baseUrl)]
      .some(value => value.toLowerCase().includes(filter)))
  const groups = [
    { title: '模型厂商', items: matching.filter(preset => !platformIds.has(preset.id) && !isVariant(preset.id)) },
    { title: '模型平台', items: matching.filter(preset => platformIds.has(preset.id)) },
    { title: '套餐与地区', items: matching.filter(preset => isVariant(preset.id)) },
  ]

  return <>
    <div className="fox-provider-catalog-toolbar">
      <div className="fox-provider-search">
        <Search aria-hidden="true" />
        <Input aria-label="搜索供应商" placeholder="搜索名称或域名" value={query} onChange={event => onQuery(event.target.value)} />
      </div>
      <Button type="button" variant="outline" onClick={() => onPick()}><Plus />自定义供应商</Button>
    </div>
    {!matching.length && <p role="status" className="fox-settings-sheet-hint">没有找到匹配的供应商。可以按名称或域名搜索，也可以选择自定义。</p>}
    {groups.filter(group => group.items.length).map(group => <section className="fox-settings-sheet-section" key={group.title}>
      <div className="fox-settings-sheet-section-head"><h3>{group.title}</h3></div>
      <div className="fox-provider-catalog-grid">
        {group.items.map(preset => <Button type="button" variant="outline" className="fox-provider-catalog-item" key={preset.id}
          aria-label={preset.name} title={preset.name} onClick={() => onPick(preset)}>
          {preset.icon ? <img src={`/providers/${preset.icon}`} alt="" /> : <Server aria-hidden="true" />}
          <span className="fox-provider-catalog-name">{preset.catalog?.label ?? preset.name}</span><ChevronRight className="size-3" aria-hidden="true" />
        </Button>)}
      </div>
    </section>)}
  </>
}
