import { forwardRef, useRef, useState, type ComponentProps } from 'react'
import { Cpu } from 'lucide-react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import { Command, CommandGroup, CommandItem, CommandList } from '@/components/ui/command'
import { ComposerMenu as UpstreamMenu, ComposerAttachmentChip as UpstreamAttachment, ComposerAttachments as UpstreamAttachments, ComposerModelTrigger } from '../upstream/elements/composer'
import './fox-elements.css'

// Standalone pieces from elements-composer, with Fox tokens and cmdk retained
// by the caller for its existing knowledge menu and keyboard selection.
export const ComposerMenu = forwardRef<HTMLDivElement, ComponentProps<'div'> & { open: boolean }>(function ComposerMenu({ open, className, ...props }, ref) {
  if (!open) return null
  return <UpstreamMenu ref={ref} open={open} className={cn('aui-composer-menu w-full max-w-none rounded-[var(--fox-radius-large)]', className)} {...props} />
})

export function ComposerAttachments(props: ComponentProps<'div'>) {
  return <UpstreamAttachments {...props} />
}

export function ComposerAttachmentChip({ name, mediaType, meta, onRemove }: { name: string; mediaType?: string; meta?: string; onRemove?: () => void }) {
  return <UpstreamAttachment className="aui-composer-attachment rounded-[var(--fox-radius-card)]" attachment={{ name, meta: meta || '已添加', state: 'done', kind: mediaType?.startsWith('image/') ? 'image' : 'text' }} onRemove={onRemove} />
}

export type ComposerModelOption = { id: string; name: string; meta?: string; icon?: string | null; disabled?: boolean }
export function Models({ value, options, disabled, onValueChange, fallbackLabel = '选择模型', note, onManage, menuRow = false }: { value?: string; options: readonly ComposerModelOption[]; disabled?: boolean; onValueChange?: (value: string) => void; fallbackLabel?: string; note?: string; onManage?: () => void; menuRow?: boolean }) {
  const [open, setOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const selected = options.find(option => option.id === value)
  const label = selected?.name || fallbackLabel
  return <Popover open={open} onOpenChange={setOpen}><PopoverTrigger asChild>{menuRow
    ? <button type="button" className="fox-expert-chip fox-history-model-trigger" disabled={disabled} title={`切换模型，当前 ${label}`}>
      <span className="fox-expert-icon" aria-hidden="true">{selected?.icon ? <img src={selected.icon} alt="" /> : <Cpu size={11} />}</span>
      <span className="min-w-0 truncate">{label}</span>
    </button>
    : <ComposerModelTrigger model={label} open={open} className="aui-composer-model-trigger rounded-[var(--fox-radius-control)]" disabled={disabled} aria-label="选择模型" />}</PopoverTrigger>
    <PopoverContent data-slot="composer-menu" className="aui-model-menu fox-composer-picker-menu" side="top" align="end" onOpenAutoFocus={event => { event.preventDefault(); menuRef.current?.focus() }}>
      <p className="aui-model-menu-heading">选择模型</p>
      {note && <p className="fox-composer-model-note">{note}</p>}
      <Command ref={menuRef} tabIndex={0} aria-label="模型列表" defaultValue={value} loop>
        <CommandList><CommandGroup>{options.map(option => <CommandItem key={option.id} value={option.id} data-checked={value === option.id} disabled={option.disabled} onSelect={() => { onValueChange?.(option.id); setOpen(false) }} className="fox-composer-model-option"><span className="fox-composer-project-option"><span>{option.name}</span>{option.meta && <small>{option.meta}</small>}</span></CommandItem>)}</CommandGroup></CommandList>
      </Command>
      {!options.length && <p className="fox-composer-model-note">暂无已配置的模型</p>}
      {onManage && <button type="button" className="fox-composer-model-manage" onClick={() => { setOpen(false); onManage() }}>管理模型</button>}
    </PopoverContent>
  </Popover>
}
