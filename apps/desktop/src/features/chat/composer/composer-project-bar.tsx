import { useState } from 'react'
import { ChevronDown, FolderOpen, Plus } from 'lucide-react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandSeparator } from '@/components/ui/command'
import type { ProjectRecord } from '@/features/conversations/model/types'
import { normalizeProjectRoot } from '../project-access-dialog-state'

export function ComposerProjectBar({ projectRoot, projects, onSelect, onAdd }: {
  projectRoot?: string | null
  projects: ProjectRecord[]
  onSelect: (project: ProjectRecord) => void
  onAdd: () => void
}) {
  const [open, setOpen] = useState(false)
  const selected = projects.find(project => normalizeProjectRoot(project.rootPath) === normalizeProjectRoot(projectRoot ?? ''))
  const name = selected?.name || projectRoot?.split(/[\\/]/).filter(Boolean).at(-1) || '选择项目'
  return <div className="fox-composer-project-bar">
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild><button type="button" className="fox-composer-project-trigger" title={projectRoot ?? '选择已有项目或添加项目'}><FolderOpen size={16} /><span>{name}</span><ChevronDown size={13} /></button></PopoverTrigger>
      <PopoverContent className="fox-composer-project-menu fox-composer-picker-menu" side="bottom" align="start">
        <Command>
          <CommandInput placeholder="查找项目…" />
          <CommandList>
            <CommandEmpty>没有匹配的项目</CommandEmpty>
            <CommandGroup heading="已有项目">{projects.filter(project => project.status === 'active').map(project => <CommandItem key={project.id} data-checked={normalizeProjectRoot(project.rootPath) === normalizeProjectRoot(projectRoot ?? '')} value={`${project.name} ${project.rootPath}`} onSelect={() => { onSelect(project); setOpen(false) }}>
              <FolderOpen size={16} /><span className="fox-composer-project-option"><span>{project.name}</span><small>{project.rootPath}</small></span>
            </CommandItem>)}</CommandGroup>
            <CommandSeparator />
          </CommandList>
        </Command>
        <button type="button" className="fox-composer-project-add" onClick={() => { setOpen(false); onAdd() }}><Plus size={16} />添加项目</button>
      </PopoverContent>
    </Popover>
  </div>
}
