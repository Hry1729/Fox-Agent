import { FileText, Library, ListTodo, Sparkles, Target } from 'lucide-react'

export const slashCommands = [
  { id: '目标', action: 'mode', modeLabel: '目标', title: '设置目标', detail: '显式进入需要持续跟踪的工作模式', Icon: Target, icon: <Target size={15} /> },
  { id: '文件', action: 'attachments', modeLabel: '文件', title: '添加文件', detail: '打开文件选择器并加入当前消息', Icon: FileText, icon: <FileText size={15} /> },
  { id: '资料库', action: 'knowledge', modeLabel: '资料库', title: '选择资料库', detail: '从远程或本机资料库中直接选择', Icon: Library, icon: <Library size={15} /> },
  { id: '计划', action: 'mode', modeLabel: '计划', title: '制订计划', detail: '先生成执行计划，再开始处理', Icon: ListTodo, icon: <ListTodo size={15} /> },
  { id: '总结', action: 'mode', modeLabel: '精简', title: '精简对话', detail: '精简当前对话并保留关键上下文', Icon: Sparkles, icon: <Sparkles size={15} /> }
]

export type SlashCommand = typeof slashCommands[number]

export function slashCommandRuntimeText(command: SlashCommand, prompt: string) {
  switch (command.id) {
    case '目标': return `/目标 ${prompt}`
    case '计划': return `/计划 ${prompt}`
    case '总结': return `请结合当前对话完成以下总结要求：\n${prompt}`
    default: return prompt
  }
}
