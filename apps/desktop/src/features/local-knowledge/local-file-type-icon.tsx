import { File as FileIcon, FileCode, FileSpreadsheet, FileText, FileType, Presentation } from 'lucide-react'
import { cn } from '@/lib/utils'

export function LocalFileTypeIcon({ extension, className }: { extension: string; className?: string }) {
  const normalized = extension.toLowerCase()
  const descriptor = normalized === 'pdf'
    ? { Icon: FileType, className: 'is-pdf' }
    : normalized === 'ppt' || normalized === 'pptx'
      ? { Icon: Presentation, className: 'is-presentation' }
      : normalized === 'xls' || normalized === 'xlsx' || normalized === 'csv'
        ? { Icon: FileSpreadsheet, className: 'is-spreadsheet' }
        : normalized === 'md' || normalized === 'markdown'
          ? { Icon: FileCode, className: 'is-text' }
          : normalized === 'txt'
            ? { Icon: FileText, className: 'is-text' }
          : normalized === 'doc' || normalized === 'docx'
            ? { Icon: FileText, className: 'is-document' }
            : { Icon: FileIcon, className: 'is-unknown' }
  const { Icon } = descriptor

  return <span className={cn('fox-local-files-file-icon', descriptor.className, className)} aria-hidden="true"><Icon size={18} strokeWidth={1.75} aria-hidden="true" /></span>
}
