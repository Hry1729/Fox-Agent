import { useState } from 'react'
import { LoaderCircle, Pencil, Shield, ShieldCheck, Trash2 } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Separator } from '@/components/ui/separator'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import type { LifecycleHookRecord } from '@/features/conversations/model/types'
import { useLifecycleHooks } from './use-lifecycle-hooks'
import { SettingsLoadError } from './settings-load-error'
import './settings-pages.css'

interface HookFormState {
  id?: string
  name: string
  event: LifecycleHookRecord['event']
  matcher: string
  action: LifecycleHookRecord['action']
  reason: string
  priority: number
}

const emptyHookForm: HookFormState = { name: '', event: 'before_tool', matcher: '*', action: 'annotate', reason: '', priority: 100 }

const hookToForm = (hook: LifecycleHookRecord): HookFormState => ({ id: hook.id, name: hook.name, event: hook.event, matcher: hook.matcher, action: hook.action, reason: hook.reason, priority: hook.priority })

/**
 * Declarative lifecycle Hooks: list, add / edit, enable switch and delete
 * (with the shared in-app confirmation). One component, used by the plugin
 * center「策略」tab and the connector settings page (sandbox.md 13.22).
 */
export function LifecycleHooksSection({ showHeader = true }: { showHeader?: boolean }) {
  const hooks = useLifecycleHooks()
  const [hookForm, setHookForm] = useState<HookFormState>({ ...emptyHookForm })
  const saveHook = async () => {
    const saved = await hooks.save({ ...hookForm, name: hookForm.name.trim(), matcher: hookForm.matcher.trim(), reason: hookForm.reason.trim(), enabled: true })
    if (!saved) return
    setHookForm({ ...emptyHookForm })
    toast.success(hookForm.id ? '生命周期 Hook 已更新' : '生命周期 Hook 已添加')
  }
  const removeHook = async (hook: LifecycleHookRecord) => {
    const confirmed = await confirmDialog({ title: '删除 Hook', description: `删除 Hook“${hook.name}”？`, confirmLabel: '删除', destructive: true })
    if (!confirmed) return
    if (await hooks.remove(hook.id)) {
      if (hookForm.id === hook.id) setHookForm({ ...emptyHookForm })
      toast.success('Hook 已删除')
    }
  }
  return (
    // A fragment, not a wrapper: the settings layout styles `main > header`.
    <>
      {showHeader && <header><span>受限策略生命周期</span><h1>声明式 Hooks</h1><p>Hook 不执行脚本，只能按工具名匹配并阻止、要求审批或向模型附加说明；每次命中都会写入审计记录。</p></header>}
      {hooks.error && <SettingsLoadError message="暂时无法读取 Hook 列表" detail={hooks.error} onRetry={() => void hooks.refresh()} />}
      <Card className="fox-settings-section fox-service-form"><div className="fox-settings-section-head"><div><h2>{hookForm.id ? '编辑 Hook' : '添加 Hook'}</h2><p>匹配器支持精确名称、逗号分组，以及前后缀通配符，例如 call_*。</p></div><ShieldCheck /></div><div className="fox-settings-form-grid"><label>名称<Input value={hookForm.name} onChange={(event) => setHookForm({ ...hookForm, name: event.target.value })} placeholder="例如 高风险工具审批" /></label><label>事件<Select value={hookForm.event} onValueChange={(event: LifecycleHookRecord['event']) => setHookForm({ ...hookForm, event, action: event === 'before_tool' ? hookForm.action : 'annotate' })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="before_tool">工具调用前</SelectItem><SelectItem value="after_tool">工具调用后</SelectItem><SelectItem value="before_run">Run 开始前</SelectItem><SelectItem value="after_run">Run 结束后</SelectItem></SelectContent></Select></label><label>匹配器<Input value={hookForm.matcher} onChange={(event) => setHookForm({ ...hookForm, matcher: event.target.value })} placeholder="run_command, call_*" /></label><label>动作<Select value={hookForm.action} onValueChange={(action: LifecycleHookRecord['action']) => setHookForm({ ...hookForm, action })}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent>{hookForm.event === 'before_tool' && <SelectItem value="block">阻止</SelectItem>}{hookForm.event === 'before_tool' && <SelectItem value="require_approval">要求审批</SelectItem>}<SelectItem value="annotate">附加说明</SelectItem></SelectContent></Select></label><label>原因 / 说明<Textarea value={hookForm.reason} onChange={(event) => setHookForm({ ...hookForm, reason: event.target.value })} placeholder="会展示在审批或模型上下文中" /></label></div><div className="fox-setting-actions">{hookForm.id && <Button variant="outline" onClick={() => setHookForm({ ...emptyHookForm })}>取消编辑</Button>}<Button disabled={!hookForm.name.trim() || !hookForm.matcher.trim() || hooks.busyId != null} onClick={() => void saveHook()}>{hooks.busyId && <LoaderCircle className="animate-spin" />}保存 Hook</Button></div></Card>
      {!hooks.loading && !hooks.error && hooks.items.length === 0 && <p className="fox-lifecycle-hooks-empty">暂无 Hook。添加后会在每次匹配的工具调用或 Run 上生效。</p>}
      <div className="fox-mcp-list">{hooks.items.map((hook) => <Card key={hook.id} className="fox-settings-section fox-mcp-card" data-hook-id={hook.id}><div className="fox-mcp-card-head"><span><Shield /></span><button className="fox-skill-summary" onClick={() => setHookForm(hookToForm(hook))}><h2>{hook.name}</h2><p>{hook.event} · {hook.matcher} · {hook.action}</p></button><Badge variant={hook.action === 'block' ? 'destructive' : 'secondary'}>{hook.action}</Badge><Switch aria-label={`启用 Hook ${hook.name}`} checked={hook.enabled} disabled={hooks.busyId === hook.id} onCheckedChange={(enabled) => void hooks.setEnabled(hook.id, enabled)} /></div><Separator /><div className="fox-mcp-meta"><span>{hook.reason || '未填写说明'}</span><div><Button variant="ghost" size="icon-sm" title="编辑" onClick={() => setHookForm(hookToForm(hook))}><Pencil /></Button><Button variant="ghost" size="icon-sm" title="删除" disabled={hooks.busyId === hook.id} onClick={() => void removeHook(hook)}><Trash2 /></Button></div></div></Card>)}</div>
    </>
  )
}
