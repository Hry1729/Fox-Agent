import { useState } from 'react'
import { Button } from '@/components/ui/button'
import type { CompanionNotes } from './api'
import { ContactChoice } from './chat-decision'

export function SharedDecision({ request, busy, onChoose, chat = false }: {
  request: CompanionNotes['sharedRequests'][number]
  busy: boolean
  onChoose: (decision: 'add' | 'replace' | 'reject', replaceMemoryId?: string) => void
  chat?: boolean
}) {
  const [choosing, setChoosing] = useState(false)
  const [selected, setSelected] = useState('')
  return <div className="fox-contact-section">
    <p>{request.content}</p><blockquote>{request.evidenceExcerpt}</blockquote>
    {chat ? <div className="fox-contact-choices">
      <ContactChoice index={0} disabled={busy} onClick={() => onChoose('add')}>记住这条</ContactChoice>
      {request.matches.length > 0 && <ContactChoice index={1} disabled={busy} onClick={() => setChoosing(true)}>替换已有记忆…</ContactChoice>}
      <ContactChoice index={request.matches.length ? 2 : 1} disabled={busy} onClick={() => onChoose('reject')}>暂不记住</ContactChoice>
    </div> : <div className="fox-contact-actions"><Button size="sm" disabled={busy} onClick={() => onChoose('add')}>记住这条</Button>
      {request.matches.length > 0 && <Button size="sm" variant="outline" disabled={busy} onClick={() => setChoosing(true)}>替换…</Button>}
      <Button size="sm" variant="ghost" disabled={busy} onClick={() => onChoose('reject')}>拒绝</Button></div>}
    {choosing && <fieldset><legend>选择要停用的旧记忆</legend>{request.matches.map(match => <label key={match.id}><input type="radio" name={'replace-' + request.id} value={match.id} checked={selected === match.id} onChange={() => setSelected(match.id)} />{match.content}</label>)}
      <Button size="sm" disabled={busy || !selected} onClick={() => onChoose('replace', selected)}>确认替换</Button>
      <Button size="sm" variant="ghost" onClick={() => { setChoosing(false); setSelected('') }}>取消</Button></fieldset>}
  </div>
}
