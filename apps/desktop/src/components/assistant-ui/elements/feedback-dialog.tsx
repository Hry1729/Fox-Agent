import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { FeedbackDialog as ElementFeedbackDialog } from '../upstream/elements/feedback-dialog'
import { inkButton } from '../upstream/elements/surfaces'
import './fox-elements.css'

const reasons = [['irrelevant', '答非所问'], ['code_error', '代码错误'], ['misunderstanding', '理解偏差'], ['other', '其他']] as const
export type FeedbackCategory = typeof reasons[number][0]
export function FeedbackDialog({ open, onOpenChange, category, onCategoryChange, note, onNoteChange, saving, onSubmit }: { open: boolean; onOpenChange: (open: boolean) => void; category: FeedbackCategory; onCategoryChange: (category: FeedbackCategory) => void; note: string; onNoteChange: (note: string) => void; saving: boolean; onSubmit: () => void }) {
  return <Dialog open={open} onOpenChange={onOpenChange}><DialogContent className="fox-feedback-dialog aui-feedback-dialog"><DialogHeader><DialogTitle className="sr-only">回答反馈</DialogTitle><DialogDescription className="sr-only">只保存分类、备注和运行标识，不保存回答正文、文件内容或推理过程。</DialogDescription></DialogHeader>
    {/* The submit affordance is deliberately not passed down: the upstream pill sits
        under the note field, which left 取消 stranded on its own row. Both actions now
        share the footer, and the primary keeps the upstream surface classes. */}
    <ElementFeedbackDialog className="max-w-none border-0 bg-transparent p-0 shadow-none" reasons={reasons.map(([, label]) => label)} selected={[reasons.find(([value]) => value === category)![1]]} singleSelection note={note} sent={false} disabled={saving} onToggleReason={label => onCategoryChange(reasons.find(([, value]) => value === label)![0])} onNoteChange={onNoteChange} />
    <DialogFooter><Button variant="outline" disabled={saving} onClick={() => onOpenChange(false)}>取消</Button>
      <button type="button" disabled={saving} onClick={onSubmit} className={cn(inkButton, 'flex h-8 items-center justify-center rounded-full px-3.5 text-xs font-medium disabled:opacity-50')}>{saving ? '保存中…' : '提交反馈'}</button>
    </DialogFooter>
  </DialogContent></Dialog>
}
