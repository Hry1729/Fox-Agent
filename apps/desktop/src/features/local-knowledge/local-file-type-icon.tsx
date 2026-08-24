import { FileQuestion, FileSpreadsheet, FileText, FileType2, NotepadText, Presentation } from 'lucide-react'
import { cn } from '@/lib/utils'

export function LocalFileTypeIcon({ extension, className }: { extension: string; className?: string }) {
  const normalized = extension.toLowerCase()
  const descriptor = normalized === 'pdf'
    ? { icon: <FileType2 />, className: 'is-pdf' }
    : normalized === 'ppt' || normalized === 'pptx'
      ? { icon: <Presentation />, className: 'is-presentation' }
      : normalized === 'xls' || normalized === 'xlsx' || normalized === 'csv'
        ? { icon: <FileSpreadsheet />, className: 'is-spreadsheet' }
        : normalized === 'md' || normalized === 'markdown' || normalized === 'txt'
          ? { icon: <NotepadText />, className: 'is-text' }
          : normalized === 'doc' || normalized === 'docx'
            ? { icon: <FileText />, className: 'is-document' }
            : { icon: <FileQuestion />, className: 'is-unknown' }

  return <span className={cn('fox-local-files-file-icon', descriptor.className, className)}>{descriptor.icon}</span>
}
