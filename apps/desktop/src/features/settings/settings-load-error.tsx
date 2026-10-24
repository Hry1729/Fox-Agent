import { FileWarning } from 'lucide-react'
import { Button } from '@/components/ui/button'

/** Inline "could not load" notice shared by the settings pages and the plugin center. */
export function SettingsLoadError({ message, detail, onRetry }: { message: string; detail: string; onRetry?: () => void }) {
  return <div role="status" className="my-3 flex items-center gap-3 rounded-lg border border-border px-4 py-3 text-muted-foreground"><FileWarning className="size-4 shrink-0" aria-hidden="true" /><span className="flex min-w-0 flex-1 flex-col gap-0.5"><span className="text-sm text-foreground">{message}</span><small className="truncate text-xs" title={detail}>{detail}</small></span>{onRetry && <Button type="button" variant="outline" size="sm" onClick={onRetry}>重试</Button>}</div>
}
