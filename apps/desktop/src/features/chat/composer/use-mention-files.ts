import { useEffect, useState } from 'react'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'

export function useMentionFiles(open: boolean, conversationId?: string, supplied?: readonly string[]) {
  const [state, setState] = useState<{ files: string[]; loading: boolean; error: string | null }>({ files: [], loading: false, error: null })
  useEffect(() => {
    if (!open || supplied) return
    if (!conversationId || !desktopRuntimeAvailable) { setState({ files: [], loading: false, error: null }); return }
    let current = true
    setState({ files: [], loading: true, error: null })
    void desktopClient.listProjectFiles(conversationId).then(files => {
      if (current) setState({ files: files.filter(file => !file.isDirectory).map(file => file.path), loading: false, error: null })
    }).catch(cause => { if (current) setState({ files: [], loading: false, error: cause instanceof Error ? cause.message : '文件列表读取失败' }) })
    return () => { current = false }
  }, [open, conversationId, supplied])
  return supplied ? { files: [...supplied], loading: false, error: null } : state
}
