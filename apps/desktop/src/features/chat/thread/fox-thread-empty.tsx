import { ConversationEmptyPrompt } from '../timeline/fox-timeline'

export function FoxThreadEmpty({ suggestions }: { suggestions?: string[] }) {
  return <ConversationEmptyPrompt suggestions={suggestions} onStart={() => {}} />
}
