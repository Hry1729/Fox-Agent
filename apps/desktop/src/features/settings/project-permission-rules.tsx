// A1: the project's "always allow" (and ask / block) command rules, revocable one by one.
import { useCallback, useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { PermissionRuleRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'

const EFFECT_LABELS: Record<PermissionRuleRecord['effect'], string> = { allow: '总是允许', ask: '总是询问', block: '禁止' }

export function ProjectPermissionRules({ projectId }: { projectId: string }) {
  const [rules, setRules] = useState<PermissionRuleRecord[]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = useCallback(async () => {
    try {
      setRules(await desktopClient.permissionRulesList(projectId))
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [projectId])
  useEffect(() => { void load() }, [load])
  const revoke = async (rule: PermissionRuleRecord) => {
    setBusy(rule.id)
    try {
      await desktopClient.permissionRuleRevoke(rule.id)
      toast.success(`已撤销规则「${rule.pattern}」`)
      await load()
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(null)
    }
  }
  return <section className="fox-settings-sheet-section">
    <div className="fox-settings-sheet-section-head"><h3>命令规则</h3></div>
    <p className="fox-settings-sheet-hint">按命令前缀匹配；复合命令（&&、;、|）的每一段都要命中。宽泛前缀不会覆盖 git push 这类高风险操作，项目外和受保护路径也不受规则放行。</p>
    {error && <p className="fox-settings-sheet-hint" role="alert">{error}</p>}
    {!error && rules.length === 0 && <p className="fox-settings-sheet-hint">还没有规则。在命令审批卡上选择「本项目总是允许」即可添加。</p>}
    <ul className="fox-permission-rules">
      {rules.map(rule => <li key={rule.id}>
        <code>{rule.pattern}</code>
        <small>{EFFECT_LABELS[rule.effect]} · {rule.tool === '*' ? '所有命令工具' : rule.tool} · {new Date(rule.createdAt).toLocaleDateString()}</small>
        <Button size="sm" variant="ghost" disabled={busy === rule.id || rule.source === 'builtin'} onClick={() => void revoke(rule)}>撤销</Button>
      </li>)}
    </ul>
  </section>
}
