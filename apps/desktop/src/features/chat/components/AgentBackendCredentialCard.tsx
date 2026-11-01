// UX batch ①: login guidance for an external backend conversation.
//
// The card (credentialCardModel) for a missing login (private HOME path,
// "打开文件夹", required key / file names, templates, "从真实 HOME 导入"), or the
// Pi configuration source. The import confirmation lists file and key NAMES
// only; the Host never sends a value to the UI.
//
// An import never silently replaces a login: a file / key the target already
// has is kept unless "覆盖已有登录" (off by default) is ticked, in which case
// the Host backs it up first. A successful import's receipt is shown ONCE, by
// the caller on the bar (settings batch, fix ②: the card used to echo it in
// its own status line, which stayed after "知道了" and never timed out);
// only an import ERROR is shown here.
import { useState, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { FoxDisclosure } from '@/components/fox-ui/fox-disclosure'
import { userFacingError } from '@/features/chat/send-failure-notice'
import {
  credentialCardModel,
  importConfirmationRows,
  unifiedLoginNote,
  type AgentBackendPermissionStatus,
  type CredentialProbe,
  type ImportItem,
  type ImportReport,
} from '@/features/conversations/model/agent-backend-onboarding'
import type { AgentModelConnection } from './AgentModelConnectionCard'

function backendName(backend: string): string {
  if (backend === 'deepseek') return 'dsh'
  if (backend === 'pi') return 'pi'
  if (backend === 'codex') return 'Codex'
  return '这个 Agent'
}

/** 普通模式只有一句人话，按缺什么来说，不带路径和错误码。 */
function loginSentence(backend: string, status: AgentModelConnection['status'] | undefined): string {
  const name = backendName(backend)
  if ((status === 'missing_address' || status === 'choose_address') && backend === 'deepseek') return `${name} 还没有登录，先在模型服务里配好 Anthropic 地址再发送。`
  if (status === 'missing_address' || status === 'choose_address') return `${name} 还没有登录，先在模型服务里配好模型地址再发送。`
  if (status === 'missing_key') return `${name} 还没有登录，先在模型服务里填好密钥再发送。`
  if (status === 'missing_provider') return `${name} 还没有登录，先在模型服务里选择供应商再发送。`
  if (status === 'verification_failed') return `${name} 的模型连接没有通过，请到模型服务里检查后再发送。`
  return `${name} 还没有登录，先在模型服务里完成配置再发送。`
}

export function AgentBackendCredentialCard({ probe, permission, onOpenFolder, onLoadCandidates, onImport, onChooseFox, developerUi = false, modelConnection = null, modelForm = null, onOpenModelSettings }: {
  probe: CredentialProbe | null
  permission?: AgentBackendPermissionStatus | null
  onOpenFolder: () => Promise<unknown>
  onLoadCandidates: () => Promise<ImportItem[]>
  /** Copy the confirmed ids; `allProjects` = "套用到我所有项目"; `overwrite` = "覆盖已有登录". */
  onImport: (selected: string[], allProjects: boolean, overwrite: boolean, reuseForNewProjects: boolean) => Promise<ImportReport>
  onChooseFox?: (remember: boolean) => Promise<unknown>
  /** 开发者模式默认展开已有登录的详情。 */
  developerUi?: boolean
  modelConnection?: AgentModelConnection | null
  /** 缺模型地址时，主按钮展开这块现有表单。 */
  modelForm?: ReactNode
  onOpenModelSettings?: () => void
}) {
  const card = credentialCardModel(probe, permission, true)
  const [candidates, setCandidates] = useState<ImportItem[] | null>(null)
  const [selected, setSelected] = useState<string[]>([])
  const [allProjects, setAllProjects] = useState(false)
  const [overwrite, setOverwrite] = useState(false)
  const [reuseForNewProjects, setReuseForNewProjects] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [showForm, setShowForm] = useState(false)
  if (!card) return null
  const run = async (action: () => Promise<void>) => {
    setBusy(true)
    try { await action() } catch (error) { setMessage(userFacingError(error instanceof Error ? error.message : String(error), developerUi, '没有完成，请重试。')) } finally { setBusy(false) }
  }
  const rows = candidates ? importConfirmationRows(candidates) : []
  const content = (
    <section className="fox-agent-backend-credential-card" data-kind={card.kind} role="note" aria-label={card.title}>
      <strong>{card.title}</strong>
      <p>{card.description}</p>
      {developerUi && unifiedLoginNote(probe, true) && <p>{unifiedLoginNote(probe, true)}</p>}
      {probe?.source?.model && <p>{probe.source.choiceRequired ? '原生配置模型' : '当前配置模型'}：{probe.source.provider ? `${probe.source.provider} / ` : ''}{probe.source.model}</p>}
      {probe?.source?.modelEndpoint && <p>模型端点：<code>{probe.source.modelEndpoint}</code></p>}
      {probe?.source?.choiceRequired && onChooseFox && <>
        <label><input type="checkbox" checked={reuseForNewProjects} onChange={event => setReuseForNewProjects(event.target.checked)} />新项目沿用本次选择</label>
        <button type="button" disabled={busy} onClick={() => run(async () => { await onChooseFox(reuseForNewProjects) })}>使用 Fox 模型服务</button>
      </>}
      <div className="fox-agent-backend-credential-home">
        <span>私有 HOME：</span><code>{card.privateHome}</code>
        <button type="button" disabled={busy} onClick={() => run(async () => { await onOpenFolder() })}>打开文件夹</button>
      </div>
      {card.requiredKeys.length > 0 && <p>需要的 key：{card.requiredKeys.join('、')}</p>}
      {card.requiredFiles.length > 0 && <p>读取的文件：{card.requiredFiles.join('、')}{card.presentFiles.length ? `（已有：${card.presentFiles.join('、')}）` : ''}</p>}
      {card.templates.map((template) => (
        <FoxDisclosure key={template.id} summary={template.title}>
          <p>{template.note}</p>
          {template.files.map((file) => (
            <div key={file.path}><code>{file.path}</code><pre>{file.content}</pre></div>
          ))}
        </FoxDisclosure>
      ))}
      {card.offerImport && (candidates === null
        ? <button type="button" disabled={busy} onClick={() => run(async () => { const items = await onLoadCandidates(); setCandidates(items); setSelected(probe?.source?.choiceRequired ? items.map(item => item.id) : []) })}>从真实 HOME 导入…</button>
        : rows.length === 0
          ? <p>真实 HOME 里没有这个后端可导入的登录文件。</p>
          : (
            <fieldset className="fox-agent-backend-import" aria-label="确认要复制的项目">
              <legend>只复制，不引用；只显示文件名和 key 名</legend>
              {rows.map((row) => (
                <label key={row.id} title={row.detail}>
                  <input
                    type="checkbox"
                    checked={selected.includes(row.id)}
                    onChange={(event) => setSelected((current) => event.target.checked ? [...current, row.id] : current.filter((id) => id !== row.id))}
                  />
                  {row.label}
                </label>
              ))}
              <label>
                <input type="checkbox" checked={allProjects} onChange={(event) => setAllProjects(event.target.checked)} />
                套用到我所有项目
              </label>
              {probe?.backend === 'pi' && !probe.source?.choiceRequired && <label>
                <input type="checkbox" checked={reuseForNewProjects} onChange={event => setReuseForNewProjects(event.target.checked)} />
                新项目自动复用这些已确认配置（来源变化后重新确认）
              </label>}
              <label title="不勾选时，已有同名文件或已有该 key 的项目会被跳过，并在回执里列出">
                <input type="checkbox" checked={overwrite} onChange={(event) => setOverwrite(event.target.checked)} />
                覆盖已有登录（先备份到私有 HOME 下的带时间戳目录）
              </label>
              <p>默认跳过已经有这些文件或 key 的项目（包括本项目），不会覆盖。</p>
              <button type="button" disabled={busy || selected.length === 0} onClick={() => run(async () => { setMessage(null); await onImport(selected, allProjects, overwrite, reuseForNewProjects); setCandidates(null) })}>复制选中的项目</button>
            </fieldset>
          ))}
      {message && <p role="alert" className="fox-agent-backend-credential-error">{message}</p>}
    </section>
  )
  if (developerUi) {
    if (card.kind === 'ready' && !modelForm) return <FoxDisclosure summary="Agent 配置来源与导入">{content}</FoxDisclosure>
    return <div className="fox-agent-login-details">{content}{modelForm}</div>
  }
  if (card.kind === 'ready' && !modelForm) return null
  const fillAddress = (modelConnection?.status === 'missing_address' || modelConnection?.status === 'choose_address') && (probe?.backend === 'deepseek' || !probe?.backend)
  const primaryLabel = fillAddress ? '填写 Anthropic 地址' : '去设置模型服务'
  const sentence = loginSentence(probe?.backend ?? '', modelConnection?.status)
  const openPrimary = () => {
    if (fillAddress) setShowForm(true)
    else onOpenModelSettings?.()
  }
  return (
    <section className="fox-agent-login-card" aria-label={sentence}>
      <p className="fox-agent-login-card-copy">{sentence}</p>
      <div className="fox-agent-login-card-actions">
        <Button type="button" size="sm" onClick={openPrimary}>{primaryLabel}</Button>
      </div>
      {showForm && modelForm}
    </section>
  )
}
