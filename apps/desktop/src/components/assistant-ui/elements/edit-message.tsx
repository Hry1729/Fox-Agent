import './fox-elements.css'
import { EditMessage as UpstreamEdit } from '../upstream/elements/edit-message'

export function EditMessage({ value, onValueChange, onSave, onCancel, saving, discardedReplies = 0 }: { value: string; onValueChange: (value: string) => void; onSave: () => void; onCancel: () => void; saving: boolean; discardedReplies?: number }) {
  return <UpstreamEdit value={value} editing discardedReplies={discardedReplies} onValueChange={onValueChange} onSave={onSave} onCancel={onCancel} disabled={saving} className="aui-edit-message max-w-none rounded-[var(--fox-radius-large)]" onKeyDown={event => {
    if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return
    if (event.key === 'Escape') { event.preventDefault(); onCancel() }
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); if (value.trim() && !saving) onSave() }
  }} />
}
