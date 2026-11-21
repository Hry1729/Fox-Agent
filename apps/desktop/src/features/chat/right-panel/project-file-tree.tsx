import { useEffect, useMemo, useState } from 'react'
import { hotkeysCoreFeature, syncDataLoaderFeature } from '@headless-tree/core'
import { useTree } from '@headless-tree/react'
import { Braces, File, FileCode, FileImage, FileSpreadsheet, FileText, Folder, FolderOpen, Palette } from 'lucide-react'
import { Tree, TreeItem, TreeItemLabel } from '@/components/reui/tree'
import type { ProjectFileEntry } from '@/features/conversations/model/types'
import { managedFilePathKey } from '../components/managed-files/model'

// Adapted from @reui/c-tree-5; project loading remains owned by useProjectFiles.
export function ProjectFileTree({ entries, workspaceName, selectedFile, onSelect, marks = {}, onDirectoriesChange }: { entries: ProjectFileEntry[]; workspaceName: string; selectedFile: string; onSelect: (path: string) => void; marks?: Record<string, string>; onDirectoriesChange?: (paths: string[]) => void }) {
  const root = '__fox_workspace_root__'
  const [expandedItems, setExpandedItems] = useState<string[]>([root])
  const nodes = useMemo(() => new Map<string, ProjectFileEntry>([
    [root, { path: root, name: workspaceName, parent: '', isDirectory: true, hasChildren: true, byteSize: 0, modifiedAt: null }],
    ...entries.map(entry => [entry.path, entry] as [string, ProjectFileEntry]),
  ]), [entries, workspaceName])
  const children = useMemo(() => {
    const result = new Map<string, string[]>()
    for (const entry of entries) {
      const parent = entry.parent || root
      result.set(parent, [...(result.get(parent) ?? []), entry.path])
    }
    return result
  }, [entries])
  const tree = useTree<ProjectFileEntry>({
    rootItemId: '__fox_tree__', indent: 18,
    state: { expandedItems }, setExpandedItems,
    getItemName: item => item.getItemData().name,
    isItemFolder: item => item.getItemData().isDirectory,
    onPrimaryAction: item => { if (!item.isFolder()) onSelect(item.getId()) },
    dataLoader: { getItem: id => nodes.get(id === '__fox_tree__' ? root : id)!, getChildren: id => id === '__fox_tree__' ? [root] : children.get(id) ?? [] },
    features: [syncDataLoaderFeature, hotkeysCoreFeature],
  })
  useEffect(() => { tree.rebuildTree() }, [tree, nodes, children, expandedItems])
  useEffect(() => { onDirectoriesChange?.(['', ...expandedItems.filter(path => path !== root)]) }, [expandedItems, onDirectoriesChange])
  useEffect(() => {
    const selected = entries.find(entry => managedFilePathKey(entry.path).toLocaleLowerCase() === managedFilePathKey(selectedFile).toLocaleLowerCase())
    const ancestors: string[] = []
    let parent = selected?.parent
    while (parent) { ancestors.push(parent); parent = nodes.get(parent)?.parent }
    setExpandedItems(current => ancestors.every(path => current.includes(path)) ? current : [...new Set([...current, ...ancestors])])
  }, [entries, nodes, selectedFile])
  return <Tree tree={tree} indent={18} className="fox-project-tree" aria-label="项目文件">
    {tree.getItems().filter(item => nodes.has(item.getId())).map(item => {
      const entry = item.getItemData()
      const selected = managedFilePathKey(entry.path).toLocaleLowerCase() === managedFilePathKey(selectedFile).toLocaleLowerCase()
      const extension = entry.name.split('.').at(-1)?.toLowerCase()
      const Icon = entry.isDirectory ? (item.isExpanded() ? FolderOpen : Folder)
        : ['ts', 'tsx', 'js', 'jsx', 'py', 'rs', 'html'].includes(extension ?? '') ? FileCode
        : extension === 'json' ? Braces : extension === 'css' ? Palette
        : ['xlsx', 'xls', 'csv'].includes(extension ?? '') ? FileSpreadsheet
        : ['png', 'jpg', 'jpeg', 'svg', 'webp'].includes(extension ?? '') ? FileImage
        : ['md', 'txt', 'doc', 'docx', 'pdf'].includes(extension ?? '') ? FileText : File
      return <TreeItem key={item.getId()} item={item} title={entry.path === root ? workspaceName : entry.path} data-selected={selected} aria-current={selected ? 'true' : undefined}>
        <TreeItemLabel><Icon className={entry.isDirectory ? 'is-folder' : 'is-file'} /><span className="fox-file-tree-name">{entry.name}</span>{marks[managedFilePathKey(entry.path)] && <small className="fox-file-tree-mark">{marks[managedFilePathKey(entry.path)]}</small>}</TreeItemLabel>
      </TreeItem>
    })}
  </Tree>
}
