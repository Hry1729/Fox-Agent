import { ComposerPrimitive, useAuiState } from '@assistant-ui/react'
import { Button } from '@/components/ui/button'

/**
 * The edit surface for the last user message (migration step 5C). The composer
 * primitives bind to the message's own edit composer: 取消 closes it, 发送
 * routes through the runtime's `onEdit` → `rerunFromMessage` (which truncates
 * later messages).
 */
export function FoxEditComposer() {
  const isEditing = useAuiState((state) => state.message.composer.isEditing)
  if (!isEditing) return null
  return (
    <ComposerPrimitive.Root className="fox-edit-composer">
      <ComposerPrimitive.Input className="fox-edit-composer-input" rows={3} autoFocus aria-label="编辑最新消息" />
      <div className="fox-edit-composer-actions">
        <ComposerPrimitive.Cancel asChild>
          <Button variant="ghost" size="sm" className="fox-edit-composer-cancel">取消</Button>
        </ComposerPrimitive.Cancel>
        <ComposerPrimitive.Send asChild>
          <Button size="sm" className="fox-edit-composer-send">
            <ArrowUpGlyph />
            发送
          </Button>
        </ComposerPrimitive.Send>
      </div>
    </ComposerPrimitive.Root>
  )
}

function ArrowUpGlyph() {
  return <svg className="fox-edit-composer-arrow" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m5 12 7-7 7 7" /><path d="M12 19V5" /></svg>
}
