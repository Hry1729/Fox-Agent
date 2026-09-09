import type { LocalKnowledgeBaseDto } from '@/features/conversations/api/desktop-client'

export function localKnowledgePickerState(item: Pick<LocalKnowledgeBaseDto,
  'documentCount' | 'activeJobStatus' | 'textIndexReady' | 'vectorIndexReady'>) {
  const available = item.textIndexReady === true || item.vectorIndexReady === true
  const jobStatus = item.activeJobStatus?.toLowerCase()
  if (available) {
    return {
      available,
      status: jobStatus === 'running' || jobStatus === 'queued'
        ? '更新中 · 现有内容可用'
        : item.vectorIndexReady ? '可用 · 向量检索' : '可用 · 关键词检索',
      reason: '',
    }
  }
  return {
    available,
    status: knowledgePickerStatus(item.activeJobStatus, false, item.documentCount),
    reason: item.documentCount > 0
      ? '文档已导入，尚无可检索内容。请在知识库页面查看解析进度或失败原因。'
      : '请先在知识库页面导入文档并等待解析完成。',
  }
}

export function knowledgePickerStatus(activeJobStatus: string | null | undefined, indexed: boolean, documentCount = 0) {
  const labels: Record<string, string> = {
    queued: '排队中', running: '处理中', paused: '已暂停', failed: '处理失败',
    error: '处理失败', cancelled: '已取消', interrupted: '已中断',
  }
  return labels[activeJobStatus?.toLowerCase() ?? ''] ?? (indexed ? '已索引' : documentCount > 0 ? '待解析' : '待导入')
}

export function filterKnowledgePickerItems<T extends { source: string; name: string; description: string; unavailable?: boolean }>(items: T[], source: string, query: string): T[] {
  const search = query.trim().toLocaleLowerCase()
  return items.filter((item) => item.source === source
    && `${item.name} ${item.description}`.toLocaleLowerCase().includes(search))
    .sort((left, right) => Number(Boolean(left.unavailable)) - Number(Boolean(right.unavailable)))
}
