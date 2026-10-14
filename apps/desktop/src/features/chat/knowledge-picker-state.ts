import type { LocalKnowledgeBaseDto } from '@/features/conversations/api/desktop-client'

export function localKnowledgePickerState(item: Pick<LocalKnowledgeBaseDto,
  'documentCount' | 'activeJobStatus' | 'textIndexReady' | 'vectorIndexReady'>) {
  const available = item.textIndexReady === true || item.vectorIndexReady === true
  const jobStatus = item.activeJobStatus?.toLowerCase()
  if (available) {
    return {
      available,
      status: jobStatus === 'running' || jobStatus === 'queued'
        ? '正在整理 · 现有内容可用'
        : item.vectorIndexReady ? '可用 · 智能搜索' : '可用 · 按字词查找',
      reason: '',
    }
  }
  return {
    available,
    status: knowledgePickerStatus(item.activeJobStatus, false, item.documentCount),
    reason: item.documentCount > 0
      ? '资料已添加，还搜不到内容。请在资料库页面查看整理进度或失败原因。'
      : '请先在资料库页面添加资料，并等待整理完成。',
  }
}

export function knowledgePickerStatus(activeJobStatus: string | null | undefined, indexed: boolean, documentCount = 0) {
  const labels: Record<string, string> = {
    queued: '排队中', running: '正在整理', paused: '已暂停', failed: '整理失败',
    error: '整理失败', cancelled: '已取消', interrupted: '已中断',
  }
  return labels[activeJobStatus?.toLowerCase() ?? ''] ?? (indexed ? '已整理' : documentCount > 0 ? '待整理' : '还没有资料')
}

export function filterKnowledgePickerItems<T extends { source: string; name: string; description: string; unavailable?: boolean }>(items: T[], source: string, query: string): T[] {
  const search = query.trim().toLocaleLowerCase()
  return items.filter((item) => item.source === source
    && `${item.name} ${item.description}`.toLocaleLowerCase().includes(search))
    .sort((left, right) => Number(Boolean(left.unavailable)) - Number(Boolean(right.unavailable)))
}
