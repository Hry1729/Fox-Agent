import { lazy, Suspense } from 'react'
import { LoadingScreen } from './LoadingScreen'
import { TooltipProvider } from '@/components/ui/tooltip'
import { ConfirmDialogProvider } from '@/components/ui/confirm-dialog'
import { PromptDialogProvider } from '@/components/ui/prompt-dialog'
const Workbench = lazy(() => import('@/features/chat/workbench').then(module => ({ default: module.Workbench })))

export function App() {
  return <TooltipProvider><Suspense fallback={<LoadingScreen />}><Workbench /></Suspense><ConfirmDialogProvider /><PromptDialogProvider /></TooltipProvider>
}
