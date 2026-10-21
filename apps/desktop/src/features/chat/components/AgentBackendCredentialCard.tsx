// UX batch ①: login guidance for an external backend conversation.
//
// Two different cards (credentialCardModel): a missing login (private HOME
// path, "打开文件夹", required key / file names, templates, "从真实 HOME 导入")
// and a loopback gateway refused in the sandbox (full access at creation, or
// the official endpoint). The import confirmation lists file and key NAMES
// only; the Host never sends a value to the UI.
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
  /** Copy the confirmed ids; `allProjects` = "套用到我所有项目". */
  onImport: (selected: string[], allProjects: boolean) => Promise<ImportReport>
}) {
  const card = credentialCardModel(probe, permission)
  const [candidates, setCandidates] = useState<ImportItem[] | null>(null)
  const [selected, setSelected] = useState<string[]>([])
  const [allProjects, setAllProjects] = useState(false)
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
              <button type="button" disabled={busy || selected.length === 0} onClick={() => run(async () => { setMessage(importReceipt(await onImport(selected, allProjects))) })}>复制选中的项目</button>
            </fieldset>
          ))}
      {message && <p role="status">{message}</p>}
    </section>
  )
}
