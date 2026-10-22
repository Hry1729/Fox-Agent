// UX batch ①: login guidance for an external backend conversation.
//
// Two different cards (credentialCardModel): a missing login (private HOME
// path, "打开文件夹", required key / file names, templates, "从真实 HOME 导入")
// and a loopback gateway refused in the sandbox (full access at creation, or
// the official endpoint). The import confirmation lists file and key NAMES
// only; the Host never sends a value to the UI.
//
// An import never silently replaces a login: a file / key the target already
// has is kept unless "覆盖已有登录" (off by default) is ticked, in which case
// the Host backs it up first. The receipt is shown by the caller OUTSIDE this
// card (a successful import makes the probe ready and the card disappears).
import { useState } from 'react'
import {
  credentialCardModel,
  importConfirmationRows,
  importReceipt,
  type AgentBackendPermissionStatus,
  type CredentialProbe,
  type ImportItem,
  type ImportReport,
} from '@/features/conversations/model/agent-backend-onboarding'

export function AgentBackendCredentialCard({ probe, permission, onOpenFolder, onLoadCandidates, onImport }: {
  probe: CredentialProbe | null
  permission?: AgentBackendPermissionStatus | null
  onOpenFolder: () => Promise<unknown>
  onLoadCandidates: () => Promise<ImportItem[]>
  /** Copy the confirmed ids; `allProjects` = "套用到我所有项目"; `overwrite` = "覆盖已有登录". */
  onImport: (selected: string[], allProjects: boolean, overwrite: boolean) => Promise<ImportReport>
}) {
  const card = credentialCardModel(probe, permission)
  const [candidates, setCandidates] = useState<ImportItem[] | null>(null)
  const [selected, setSelected] = useState<string[]>([])
  const [allProjects, setAllProjects] = useState(false)
  const [overwrite, setOverwrite] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  if (!card) return null
  const run = async (action: () => Promise<void>) => {
    setBusy(true)
    try { await action() } catch (error) { setMessage(error instanceof Error ? error.message : String(error)) } finally { setBusy(false) }
  }
  const rows = candidates ? importConfirmationRows(candidates) : []
  return (
    <section className="fox-agent-backend-credential-card" data-kind={card.kind} role="note" aria-label={card.title}>
      <strong>{card.title}</strong>
      <p>{card.description}</p>
      <div className="fox-agent-backend-credential-home">
        <span>私有 HOME：</span><code>{card.privateHome}</code>
        <button type="button" disabled={busy} onClick={() => run(async () => { await onOpenFolder() })}>打开文件夹</button>
      </div>
      {card.requiredKeys.length > 0 && <p>需要的 key：{card.requiredKeys.join('、')}</p>}
      {card.requiredFiles.length > 0 && <p>读取的文件：{card.requiredFiles.join('、')}{card.presentFiles.length ? `（已有：${card.presentFiles.join('、')}）` : ''}</p>}
      {card.templates.map((template) => (
        <details key={template.id}>
          <summary>{template.title}</summary>
          <p>{template.note}</p>
          {template.files.map((file) => (
            <div key={file.path}><code>{file.path}</code><pre>{file.content}</pre></div>
          ))}
        </details>
      ))}
      {card.offerImport && (candidates === null
        ? <button type="button" disabled={busy} onClick={() => run(async () => { const items = await onLoadCandidates(); setCandidates(items); setSelected([]) })}>从真实 HOME 导入…</button>
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
              <label title="不勾选时，已有同名文件或已有该 key 的项目会被跳过，并在回执里列出">
                <input type="checkbox" checked={overwrite} onChange={(event) => setOverwrite(event.target.checked)} />
                覆盖已有登录（先备份到私有 HOME 下的带时间戳目录）
              </label>
              <p>默认跳过已经有这些文件或 key 的项目（包括本项目），不会覆盖。</p>
              <button type="button" disabled={busy || selected.length === 0} onClick={() => run(async () => { setMessage(importReceipt(await onImport(selected, allProjects, overwrite))) })}>复制选中的项目</button>
            </fieldset>
          ))}
      {message && <p role="status">{message}</p>}
    </section>
  )
}
