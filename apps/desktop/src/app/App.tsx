import { Workbench } from '@/features/chat/workbench'
import { TooltipProvider } from '@/components/ui/tooltip'
import { Toaster } from '@/components/ui/sonner'

export function App() {
  return (
    <TooltipProvider>
      <Workbench />
      <Toaster position="bottom-center" />
    </TooltipProvider>
  )
}
