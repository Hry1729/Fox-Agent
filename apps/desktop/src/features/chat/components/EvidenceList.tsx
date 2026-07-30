import { AlertTriangle, CheckCircle, Clock, FileText, XCircle } from 'lucide-react'
import type { TaskEvidenceRecord } from '@/features/conversations/model/types'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'

interface EvidenceListProps {
  evidence: TaskEvidenceRecord[]
  onEvidenceClick?: (evidence: TaskEvidenceRecord) => void
}

function getValidityIcon(status: TaskEvidenceRecord['validityStatus']) {
  switch (status) {
    case 'valid':
      return <CheckCircle className="h-3 w-3 text-green-600" />
    case 'stale':
      return <Clock className="h-3 w-3 text-yellow-600" />
    case 'missing':
      return <XCircle className="h-3 w-3 text-red-600" />
    case 'invalid':
      return <AlertTriangle className="h-3 w-3 text-red-600" />
    case 'unverified':
    default:
      return <Clock className="h-3 w-3 text-gray-400" />
  }
}

function getValidityLabel(status: TaskEvidenceRecord['validityStatus']) {
  switch (status) {
    case 'valid':
      return '有效'
    case 'stale':
      return '已过期'
    case 'missing':
      return '缺失'
    case 'invalid':
      return '无效'
    case 'unverified':
    default:
      return '未验证'
  }
}

function getEvidenceTypeLabel(type: TaskEvidenceRecord['evidenceType']) {
  switch (type) {
    case 'tool_call':
      return '工具调用'
    case 'trace_span':
      return '执行跟踪'
    case 'test_result':
      return '测试结果'
    case 'file_diff':
      return '文件变更'
    case 'artifact':
      return 'Artifact'
    case 'user_confirmation':
      return '用户确认'
    case 'external_reference':
      return '外部引用'
    default:
      return type
  }
}

export function EvidenceList({ evidence, onEvidenceClick }: EvidenceListProps) {
  if (evidence.length === 0) {
    return (
      <div className="text-xs text-muted-foreground px-2 py-1">
        无证据
      </div>
    )
  }

  return (
    <div className="space-y-1">
      {evidence.map((ev) => (
        <div
          key={ev.id}
          className={cn(
            'flex items-start gap-2 px-2 py-1.5 rounded text-xs',
            onEvidenceClick && 'cursor-pointer hover:bg-accent/50 transition-colors'
          )}
          onClick={() => onEvidenceClick?.(ev)}
        >
          <div className="flex-shrink-0 mt-0.5">
            {getValidityIcon(ev.validityStatus)}
          </div>
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2 mb-0.5">
              <Badge variant="outline" className="text-[10px] px-1 py-0">
                {getEvidenceTypeLabel(ev.evidenceType)}
              </Badge>
              <span className="text-[10px] text-muted-foreground">
                {getValidityLabel(ev.validityStatus)}
              </span>
            </div>
            <div className="text-foreground/90 line-clamp-2">
              {ev.summary}
            </div>
            {ev.invalidReason && (
              <div className="text-red-600 mt-0.5 text-[10px]">
                {ev.invalidReason}
              </div>
            )}
          </div>
        </div>
      ))}
    </div>
  )
}
