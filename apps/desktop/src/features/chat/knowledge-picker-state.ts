export function knowledgePickerStatus(activeJobStatus: string | null | undefined, indexed: boolean) {
  const labels: Record<string, string> = {
    queued: '排队中', running: '处理中', paused: '已暂停', failed: '处理失败',
    error: '处理失败', cancelled: '已取消', interrupted: '已中断',
  }
  return labels[activeJobStatus?.toLowerCase() ?? ''] ?? (indexed ? '已索引' : '待导入')
}

export function filterKnowledgePickerItems<T extends { source: string; name: string; description: string; unavailable?: boolean }>(items: T[], source: string, query: string): T[] {
  const search = query.trim().toLocaleLowerCase()
  return items.filter((item) => item.source === source
    && `${item.name} ${item.description}`.toLocaleLowerCase().includes(search))
    .sort((left, right) => Number(Boolean(left.unavailable)) - Number(Boolean(right.unavailable)))
}
