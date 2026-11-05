import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { notify as toast } from '@/features/notifications'
import { AGENT_NAMES, configErrorText } from './agent-config-copy'
import type { ExternalAgentId, ImportPreview, ImportScope } from './agent-config-types'

const SCOPES: Record<ImportScope, string> = { all_and_new: '所有现有项目，并在新项目中继续使用', all_existing: '所有现有项目', default_home: '只在不选项目的对话中使用' }
export function AgentImportPreview({ backend, developer, onDone, onCancel }: { backend: ExternalAgentId; developer: boolean; onDone: () => void; onCancel: () => void }) {
  const [preview, setPreview] = useState<ImportPreview | null>(null)
  const [selected, setSelected] = useState<string[]>([])
  const [scope, setScope] = useState<ImportScope>('all_and_new')
  const [overwrite, setOverwrite] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [revision, setRevision] = useState(0)
  useEffect(() => {
    let active = true
    setPreview(null); setError('')
    void desktopClient.agentImportPreview(backend, developer).then(value => { if (active) { setPreview(value); setSelected(value.items.filter(i => i.defaultSelected).map(i => i.id)) } }).catch(cause => { if (active) setError(configErrorText(cause)) })
    return () => { active = false }
  }, [backend, developer, revision])
  const apply = async () => {
    setBusy(true); setError('')
    try {
      const result = await desktopClient.agentImportApply({ backend, selected, scope, onConflict: overwrite ? 'overwrite_with_backup' : 'keep', developer })
      toast.success(`导入完成：写入 ${result.written} 项，保留 ${result.kept} 项，备份 ${result.backups} 项。`)
      onDone()
    } catch (cause) { setError(configErrorText(cause)) }
    finally { setBusy(false) }
  }
  return <section className="fox-agent-inline" aria-label={`导入 ${AGENT_NAMES[backend]} 配置`}>
    <h3>从这台电脑导入 {AGENT_NAMES[backend]} 配置</h3>
    {!preview && !error && <p role="status">正在读取可导入内容…</p>}
    {preview && <>
      <fieldset><legend>要导入的内容</legend>{preview.items.map(item => <label className="fox-model-choice" key={item.id}><input type="checkbox" checked={selected.includes(item.id)} onChange={e => setSelected(e.target.checked ? [...selected, item.id] : selected.filter(id => id !== item.id))} />{item.label}{developer && item.devPath && <code>{item.devPath}</code>}</label>)}</fieldset>
      <fieldset><legend>用在哪里</legend>{preview.scopes.map(item => <label className="fox-model-choice" key={item.id}><input type="radio" name={`scope-${backend}`} checked={scope === item.id} onChange={() => setScope(item.id)} />{SCOPES[item.id]}（{item.projectCount} 个项目）</label>)}</fieldset>
      {preview.sourceChanged && <p>电脑上的配置已变化。本次导入将使用预览中的新配置。</p>}
      {preview.conflicts.length > 0 && <fieldset><legend>已有配置</legend><p>{preview.conflicts.map(c => c.projectName).join('、')} 已有登录或模型配置。</p><label className="fox-model-choice"><input type="radio" name={`conflict-${backend}`} checked={!overwrite} onChange={() => setOverwrite(false)} />保留它们（推荐）</label><label className="fox-model-choice"><input type="radio" name={`conflict-${backend}`} checked={overwrite} onChange={() => setOverwrite(true)} />覆盖，先备份原文件</label></fieldset>}
    </>}
    {error && <p role="alert" className="fox-model-error">{error}<Button type="button" variant="ghost" onClick={() => setRevision(r => r + 1)}>重新预览</Button></p>}
    <div className="fox-model-actions"><Button type="button" variant="outline" disabled={busy} onClick={onCancel}>取消</Button><Button type="button" disabled={!preview || !selected.length || busy} onClick={() => void apply()}>{busy ? '导入中…' : '导入'}</Button></div>
  </section>
}
