import { createContext, useContext, useState, type ReactElement } from 'react'
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuSub, ContextMenuSubContent, ContextMenuSubTrigger, ContextMenuTrigger } from '@/components/ui/context-menu'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { ArtifactActionResponse, ArtifactApplication, ArtifactRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { artifactRelativePath, displayArtifactPath } from './final-artifact-references'
import { copyTextWithFeedback } from './fox-shared'

export const ArtifactCardActionsContext = createContext<{
  projectRoot?: string | null
  onAttachFile?: (path: string) => void
}>({})

export function ArtifactCardMenu({ artifact, children }: { artifact: ArtifactRecord; children: ReactElement }) {
  const { projectRoot, onAttachFile } = useContext(ArtifactCardActionsContext)
  const [applications, setApplications] = useState<ArtifactApplication[]>([])
  const relativePath = artifactRelativePath(artifact, projectRoot)
  const runAction = async (action: Extract<ArtifactActionResponse['action'], 'open' | 'open_with' | 'reveal'>, applicationId?: string) => {
    try {
      if (relativePath) await desktopClient.projectFileAction(artifact.conversationId, relativePath, action, applicationId)
      else await desktopClient.artifactAction(artifact.conversationId, artifact.id, action, applicationId)
    } catch (cause) {
      toast.error('文件操作未完成', { description: desktopErrorDetails(cause).message })
    }
  }
  return <ContextMenu onOpenChange={open => {
    if (!open) return
    const inspection = relativePath
      ? desktopClient.projectFileAction(artifact.conversationId, relativePath, 'inspect')
      : desktopClient.inspectArtifact(artifact.conversationId, artifact.id)
    void inspection
      .then(result => setApplications(result.applications))
      .catch(() => setApplications([]))
  }}>
    <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
    <ContextMenuContent aria-label="文件操作">
      {relativePath && onAttachFile && <ContextMenuItem onSelect={() => onAttachFile(relativePath)}>添加到上下文</ContextMenuItem>}
      <ContextMenuSub>
        <ContextMenuSubTrigger>复制</ContextMenuSubTrigger>
        <ContextMenuSubContent>
          <ContextMenuItem onSelect={() => void copyTextWithFeedback(displayArtifactPath(artifact.storagePath), '已复制绝对路径')}>绝对路径</ContextMenuItem>
          {relativePath && <ContextMenuItem onSelect={() => void copyTextWithFeedback(relativePath, '已复制相对路径')}>相对路径</ContextMenuItem>}
          <ContextMenuItem onSelect={() => void copyTextWithFeedback(artifact.displayName, '已复制文件名')}>文件名</ContextMenuItem>
        </ContextMenuSubContent>
      </ContextMenuSub>
      <ContextMenuSeparator />
      <ContextMenuSub>
        <ContextMenuSubTrigger>打开方式</ContextMenuSubTrigger>
        <ContextMenuSubContent>
          {applications.map(application => <ContextMenuItem key={application.id} onSelect={() => void runAction('open_with', application.id)}>{application.label}</ContextMenuItem>)}
          {applications.length > 0 && <ContextMenuSeparator />}
          <ContextMenuItem onSelect={() => void runAction('open')}>系统默认应用</ContextMenuItem>
        </ContextMenuSubContent>
      </ContextMenuSub>
      <ContextMenuItem onSelect={() => void runAction('reveal')}>在文件资源管理器中定位</ContextMenuItem>
    </ContextMenuContent>
  </ContextMenu>
}
