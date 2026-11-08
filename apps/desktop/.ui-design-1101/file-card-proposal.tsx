import { ArrowUpRight } from 'lucide-react'
import { File } from '../src/components/assistant-ui/upstream/elements/file'
import { Button } from '../src/components/ui/button'
import './file-card-proposal.css'

export function FileCardProposal({ onOpen }: { onOpen: () => void }) {
  return (
    <Button
      type="button"
      variant="outline"
      className="design-file-card"
      aria-label="打开 p0.txt，TXT，5 B"
      onClick={onOpen}
    >
      <File.Icon mimeType="text/plain" className="design-file-card-icon" aria-hidden="true" />
      <span className="design-file-card-content">
        <File.Name className="design-file-card-name">p0.txt</File.Name>
        <span className="design-file-card-meta">
          <span>TXT</span>
          <span aria-hidden="true">·</span>
          <File.Size bytes={5} />
        </span>
      </span>
      <ArrowUpRight className="design-file-card-open size-4" aria-hidden="true" />
    </Button>
  )
}
