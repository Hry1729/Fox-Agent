import { ArrowLeft, ArrowRight } from 'lucide-react'
import { Button } from '@/components/ui/button'

export function DecisionPagination({ page, total, item, disabled, onPage }: {
  page: number; total: number; item: '题' | '项审批'; disabled: boolean; onPage: (page: number) => void
}) {
  if (total <= 1) return null
  const current = page + 1
  return <nav className="fox-decision-pagination" aria-label={item === '题' ? '问题分页' : '审批分页'}>
    <Button type="button" size="icon-xs" variant="ghost" aria-label={item === '题' ? '上一题' : '上一项审批'} disabled={disabled || page === 0} onClick={() => onPage(page - 1)}>
      <ArrowLeft aria-hidden="true" />
    </Button>
    <span role="status" aria-label={`第 ${current} ${item}，共 ${total} ${item}`}>{String(current).padStart(2, '0')}/{String(total).padStart(2, '0')}</span>
    <Button type="button" size="icon-xs" variant="ghost" aria-label={item === '题' ? '下一题' : '下一项审批'} disabled={disabled || current === total} onClick={() => onPage(page + 1)}>
      <ArrowRight aria-hidden="true" />
    </Button>
  </nav>
}
