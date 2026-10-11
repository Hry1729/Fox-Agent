import { ArrowUpRight } from 'lucide-react'
import type { ComponentProps } from 'react'
import { cn } from '@/lib/utils'
import './fox-elements.css'
import { File as UpstreamFile } from '../upstream/elements/file'

// File AUI's icon/name/size composition. Persisted Fox files are Host IDs, not
// browser URLs, so the caller supplies its Host preview action instead of href.
export function File({ name, mediaType, bytes, canOpen, className, ...props }: ComponentProps<'div'> & { name: string; mediaType?: string | null; bytes?: number | null; canOpen?: boolean }) {
  return <UpstreamFile.Root variant="outline" className={cn('aui-file-root', className)} {...props}><UpstreamFile.Icon mimeType={mediaType || 'application/octet-stream'} /><span className="aui-file-content"><UpstreamFile.Name title={name}>{name}</UpstreamFile.Name>{bytes != null && <UpstreamFile.Size bytes={bytes} className="text-xs" />}</span>{canOpen && <ArrowUpRight className="aui-file-open-icon" size={16} aria-hidden="true" />}</UpstreamFile.Root>
}
