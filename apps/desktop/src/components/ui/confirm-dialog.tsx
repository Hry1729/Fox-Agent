import * as React from 'react'

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'

/**
 * In-app confirmation (replaces the native confirm box, whose title Tauri fills
 * with the dev server address such as 127.0.0.1:1421).
 *
 * `confirmDialog(...)` resolves `true` only when the user presses the confirm
 * button. Cancel, Escape, closing the dialog, or no mounted
 * `<ConfirmDialogProvider />` all resolve `false`, so a caller that awaits it
 * never runs the destructive branch by accident.
 */
export type ConfirmDialogOptions = {
  /** Short Chinese title, e.g. 「删除服务」. */
  title: string
  description?: React.ReactNode
  /** Defaults to 「确定」. */
  confirmLabel?: string
  /** Defaults to 「取消」. */
  cancelLabel?: string
  /** Destructive styling for the confirm button. */
  destructive?: boolean
}

export type ConfirmDialogPresenter = (options: ConfirmDialogOptions) => Promise<boolean>

let presenter: ConfirmDialogPresenter | null = null

/**
 * Install the function that shows the dialog. The provider installs itself;
 * tests may install a stub. Returns a function that removes it again (only if
 * it is still the installed one).
 */
export function setConfirmDialogPresenter(next: ConfirmDialogPresenter | null): () => void {
  presenter = next
  return () => {
    if (presenter === next) presenter = null
  }
}

export function confirmDialog(options: ConfirmDialogOptions): Promise<boolean> {
  const show = presenter
  if (!show) {
    console.warn('confirmDialog: no ConfirmDialogProvider is mounted; treating as cancelled')
    return Promise.resolve(false)
  }
  return show(options).then((answer) => answer === true, () => false)
}

type PendingConfirm = {
  id: number
  options: ConfirmDialogOptions
  resolve: (answer: boolean) => void
}

/** Mount once near the root. Requests are shown one at a time, in order. */
export function ConfirmDialogProvider() {
  const [queue, setQueue] = React.useState<PendingConfirm[]>([])
  const nextId = React.useRef(0)
  const queueRef = React.useRef<PendingConfirm[]>([])
  queueRef.current = queue

  React.useEffect(() => {
    const remove = setConfirmDialogPresenter(
      (options) =>
        new Promise<boolean>((resolve) => {
          nextId.current += 1
          const id = nextId.current
          setQueue((items) => [...items, { id, options, resolve }])
        }),
    )
    return () => {
      remove()
      // Unmounting answers every open request with "cancel".
      for (const item of queueRef.current) item.resolve(false)
    }
  }, [])

  const current = queue[0]
  const settle = React.useCallback((id: number, answer: boolean) => {
    setQueue((items) => {
      const item = items.find((entry) => entry.id === id)
      if (!item) return items
      item.resolve(answer)
      return items.filter((entry) => entry.id !== id)
    })
  }, [])

  if (!current) return null
  const { options } = current
  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        if (!open) settle(current.id, false)
      }}
    >
      <AlertDialogContent data-testid="fox-confirm-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>{options.title}</AlertDialogTitle>
          {options.description != null && options.description !== '' && (
            <AlertDialogDescription className="whitespace-pre-line">{options.description}</AlertDialogDescription>
          )}
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={() => settle(current.id, false)}>{options.cancelLabel ?? '取消'}</AlertDialogCancel>
          <AlertDialogAction
            variant={options.destructive ? 'destructive' : 'default'}
            onClick={() => settle(current.id, true)}
          >
            {options.confirmLabel ?? '确定'}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
