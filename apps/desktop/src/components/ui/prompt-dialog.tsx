import * as React from 'react'

import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'

/**
 * In-app text prompt (replaces the native prompt box, whose title Tauri fills
 * with the dev server address such as 127.0.0.1:1421). Same shape as
 * `confirmDialog` in `confirm-dialog.tsx`.
 *
 * `promptDialog(...)` resolves the trimmed text only when the user confirms
 * (button or Enter). Cancel, Escape, closing the dialog, or no mounted
 * `<PromptDialogProvider />` all resolve `null`. An empty or whitespace-only
 * value cannot be confirmed.
 */
export type PromptDialogOptions = {
  /** Short Chinese title, e.g. 「重命名服务」. */
  title: string
  description?: React.ReactNode
  /** Accessible label of the text field, e.g. 「服务名称」. */
  label?: string
  /** Prefilled value; it is selected when the dialog opens. */
  defaultValue?: string
  placeholder?: string
  /** Defaults to 「确定」. */
  confirmLabel?: string
  /** Defaults to 「取消」. */
  cancelLabel?: string
}

export type PromptDialogPresenter = (options: PromptDialogOptions) => Promise<string | null>

let presenter: PromptDialogPresenter | null = null

/**
 * Install the function that shows the dialog. The provider installs itself;
 * tests may install a stub. Returns a function that removes it again (only if
 * it is still the installed one).
 */
export function setPromptDialogPresenter(next: PromptDialogPresenter | null): () => void {
  presenter = next
  return () => {
    if (presenter === next) presenter = null
  }
}

export function promptDialog(options: PromptDialogOptions): Promise<string | null> {
  const show = presenter
  if (!show) {
    console.warn('promptDialog: no PromptDialogProvider is mounted; treating as cancelled')
    return Promise.resolve(null)
  }
  return show(options).then(
    (answer) => {
      if (typeof answer !== 'string') return null
      const trimmed = answer.trim()
      return trimmed ? trimmed : null
    },
    () => null,
  )
}

type PendingPrompt = {
  id: number
  options: PromptDialogOptions
  resolve: (answer: string | null) => void
}

function PromptDialogBody({ request, onSettle }: { request: PendingPrompt; onSettle: (answer: string | null) => void }) {
  const { options } = request
  const [value, setValue] = React.useState(options.defaultValue ?? '')
  const inputRef = React.useRef<HTMLInputElement>(null)
  const canConfirm = value.trim().length > 0
  const submit = (event?: React.FormEvent) => {
    event?.preventDefault()
    if (!canConfirm) return
    onSettle(value.trim())
  }
  return (
    <DialogContent
      data-testid="fox-prompt-dialog"
      onOpenAutoFocus={(event) => {
        event.preventDefault()
        inputRef.current?.focus()
        inputRef.current?.select()
      }}
    >
      <form className="grid gap-4" onSubmit={submit}>
        <DialogHeader>
          <DialogTitle>{options.title}</DialogTitle>
          {options.description != null && options.description !== '' && (
            <DialogDescription className="whitespace-pre-line">{options.description}</DialogDescription>
          )}
        </DialogHeader>
        <Input
          ref={inputRef}
          aria-label={options.label ?? options.title}
          placeholder={options.placeholder}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            // Enter confirms, except while an IME composition (e.g. Chinese
            // pinyin) is using Enter to commit its candidate.
            if (event.key !== 'Enter' || event.nativeEvent.isComposing || event.keyCode === 229) return
            event.preventDefault()
            submit()
          }}
          aria-invalid={!canConfirm || undefined}
        />
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onSettle(null)}>{options.cancelLabel ?? '取消'}</Button>
          <Button type="submit" disabled={!canConfirm}>{options.confirmLabel ?? '确定'}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  )
}

/** Mount once near the root. Requests are shown one at a time, in order. */
export function PromptDialogProvider() {
  const [queue, setQueue] = React.useState<PendingPrompt[]>([])
  const nextId = React.useRef(0)
  const queueRef = React.useRef<PendingPrompt[]>([])
  queueRef.current = queue

  React.useEffect(() => {
    const remove = setPromptDialogPresenter(
      (options) =>
        new Promise<string | null>((resolve) => {
          nextId.current += 1
          const id = nextId.current
          setQueue((items) => [...items, { id, options, resolve }])
        }),
    )
    return () => {
      remove()
      // Unmounting answers every open request with "cancel".
      for (const item of queueRef.current) item.resolve(null)
    }
  }, [])

  const settle = React.useCallback((id: number, answer: string | null) => {
    setQueue((items) => {
      const item = items.find((entry) => entry.id === id)
      if (!item) return items
      item.resolve(answer)
      return items.filter((entry) => entry.id !== id)
    })
  }, [])

  const current = queue[0]
  if (!current) return null
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        // Escape, the close button and an outside click all land here.
        if (!open) settle(current.id, null)
      }}
    >
      <PromptDialogBody key={current.id} request={current} onSettle={(answer) => settle(current.id, answer)} />
    </Dialog>
  )
}
