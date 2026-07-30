import { useState } from 'react'
import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  Circle,
  Loader2,
  Minus,
  PauseCircle
} from 'lucide-react'
import type { WorkTaskRecord, TaskEvidenceRecord } from '@/features/conversations/model/types'
import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { EvidenceList } from './EvidenceList'

interface TaskItemProps {
  task: WorkTaskRecord
  evidence: TaskEvidenceRecord[]
  onEvidenceClick?: (evidence: TaskEvidenceRecord) => void
}

function getTaskStatusIcon(status: WorkTaskRecord['status']) {
  switch (status) {
    case 'queued':
      return <Circle className="h-4 w-4 text-gray-400" />
    case 'in_progress':
      return <Loader2 className="h-4 w-4 text-blue-600 animate-spin" />
    case 'completed':
      return <Check className="h-4 w-4 text-green-600" />
    case 'blocked':
      return <AlertTriangle className="h-4 w-4 text-yellow-600" />
    case 'interrupted':
      return <PauseCircle className="h-4 w-4 text-orange-600" />
    case 'skipped':
      return <Minus className="h-4 w-4 text-gray-400" />
    default:
      return <Circle className="h-4 w-4 text-gray-400" />
  }
}

function getTaskStatusLabel(status: WorkTaskRecord['status']) {
  switch (status) {
    case 'queued':
      return '排队中'
    case 'in_progress':
      return '进行中'
    case 'completed':
      return '已完成'
    case 'blocked':
      return '已阻塞'
    case 'interrupted':
      return '已中断'
    case 'skipped':
      return '已跳过'
    default:
      return status
  }
}

export function TaskItem({ task, evidence, onEvidenceClick }: TaskItemProps) {
  const [isExpanded, setIsExpanded] = useState(false)
  const hasEvidence = evidence.length > 0
  const hasDetails = task.detail || task.blockedReason || hasEvidence

  return (
    <Collapsible open={isExpanded} onOpenChange={setIsExpanded}>
      <div className={cn(
        'border rounded-lg overflow-hidden',
        task.status === 'completed' && 'border-green-200 bg-green-50/50',
        task.status === 'in_progress' && 'border-blue-200 bg-blue-50/50',
        task.status === 'blocked' && 'border-yellow-200 bg-yellow-50/50',
        task.status === 'interrupted' && 'border-orange-200 bg-orange-50/50'
      )}>
        <div className="flex items-start gap-3 p-3">
          <div className="flex-shrink-0 mt-0.5">
            {getTaskStatusIcon(task.status)}
          </div>

          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2 mb-1">
              <h4 className="text-sm font-medium text-foreground">
                {task.title}
              </h4>
              <Badge variant="outline" className="text-[10px] px-1.5 py-0">
                {getTaskStatusLabel(task.status)}
              </Badge>
              {task.attempt > 1 && (
                <Badge variant="secondary" className="text-[10px] px-1.5 py-0">
                  尝试 {task.attempt}
                </Badge>
              )}
            </div>

            {task.blockedReason && (
              <div className="text-xs text-yellow-700 bg-yellow-100 px-2 py-1 rounded mb-2">
                <span className="font-medium">阻塞原因: </span>
                {task.blockedReason}
              </div>
            )}

            {hasEvidence && (
              <div className="text-xs text-muted-foreground mb-1">
                {evidence.length} 个证据
              </div>
            )}
          </div>

          {hasDetails && (
            <CollapsibleTrigger asChild>
              <button className="flex-shrink-0 p-1 hover:bg-accent rounded transition-colors">
                {isExpanded ? (
                  <ChevronDown className="h-4 w-4 text-muted-foreground" />
                ) : (
                  <ChevronRight className="h-4 w-4 text-muted-foreground" />
                )}
              </button>
            </CollapsibleTrigger>
          )}
        </div>

        {hasDetails && (
          <CollapsibleContent>
            <div className="border-t px-3 py-2 space-y-2">
              {task.detail && (
                <div className="text-xs text-muted-foreground">
                  <div className="font-medium mb-1">详情:</div>
                  <div className="text-foreground/80">{task.detail}</div>
                </div>
              )}

              {hasEvidence && (
                <div className="text-xs">
                  <div className="font-medium mb-1 text-muted-foreground">证据:</div>
                  <EvidenceList evidence={evidence} onEvidenceClick={onEvidenceClick} />
                </div>
              )}
            </div>
          </CollapsibleContent>
        )}
      </div>
    </Collapsible>
  )
}
