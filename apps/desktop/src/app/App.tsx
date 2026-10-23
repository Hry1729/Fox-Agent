import { lazy, Suspense } from 'react'
import { TooltipProvider } from '@/components/ui/tooltip'
import { ConfirmDialogProvider } from '@/components/ui/confirm-dialog'
const Workbench = lazy(() => import('@/features/chat/workbench').then(module => ({ default: module.Workbench })))

export function App() {
  return <TooltipProvider><Suspense fallback={<div className="fox-app-loading" role="status" aria-label="正在加载工作区" style={{ minHeight: '100dvh' }} />}><Workbench /></Suspense><ConfirmDialogProvider /></TooltipProvider>
}
