import { cn } from '@/lib/utils'

export function LocalFileTypeIcon({ extension, className }: { extension: string; className?: string }) {
  const normalized = extension.toLowerCase()
  const descriptor = normalized === 'pdf'
    ? { src: '/file-icons/pdf.svg', className: 'is-pdf' }
    : normalized === 'ppt' || normalized === 'pptx'
      ? { src: '/file-icons/ppt.svg', className: 'is-presentation' }
      : normalized === 'xls' || normalized === 'xlsx' || normalized === 'csv'
        ? { src: '/file-icons/spreadsheet.svg', className: 'is-spreadsheet' }
        : normalized === 'md' || normalized === 'markdown'
          ? { src: '/file-icons/markdown.svg', className: 'is-text' }
          : normalized === 'txt'
            ? { src: '/file-icons/text.svg', className: 'is-text' }
          : normalized === 'doc' || normalized === 'docx'
            ? { src: '/file-icons/word.svg', className: 'is-document' }
            : { src: '/file-icons/file.svg', className: 'is-unknown' }

  return <span className={cn('fox-local-files-file-icon', descriptor.className, className)} aria-hidden="true"><img src={descriptor.src} alt="" /></span>
}
