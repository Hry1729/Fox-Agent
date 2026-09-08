import { Workbench } from '@/features/chat/workbench'
import { TooltipProvider } from '@/components/ui/tooltip'

export function App() {
  return (
    <TooltipProvider>
      <Workbench />
    </TooltipProvider>
  )
}
