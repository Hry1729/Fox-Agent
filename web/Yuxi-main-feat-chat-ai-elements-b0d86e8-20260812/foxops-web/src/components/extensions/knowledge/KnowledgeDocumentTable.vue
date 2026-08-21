<template>
  <div class="knowledge-document-table">
    <!-- 文件管理顶栏 -->
    <div class="file-management-info">
      <div class="file-toolbar-left">
        <button
          type="button"
          class="panel-action panel-action-primary"
          @click="uploadVisible = true"
        >
          <Upload :size="14" />
          <span>上传文件</span>
        </button>
        <button type="button" class="panel-action panel-action-secondary" @click="createFolder">
          <FolderPlus :size="14" />
          <span>新建文件夹</span>
        </button>
        <ElInput
          v-model="search"
          clearable
          placeholder="搜索文件名"
          class="file-search"
          @keyup.enter="loadDocuments"
          @clear="loadDocuments"
        />
      </div>

      <div class="file-panel-status">
        <button
          v-if="pendingParseCount > 0"
          type="button"
          class="file-stat-card file-stat-action file-stat-parse"
          :disabled="actionLoading"
          @click="confirmBatchParse"
        >
          <FileText :size="16" />
          <div class="file-stat-inline">
            <strong>{{ pendingParseCount }}</strong>
            <span>待解析</span>
          </div>
        </button>
        <button
          v-if="pendingIndexCount > 0"
          type="button"
          class="file-stat-card file-stat-action file-stat-index"
          :disabled="actionLoading"
          @click="openPendingIndex"
        >
          <Database :size="16" />
          <div class="file-stat-inline">
            <strong>{{ pendingIndexCount }}</strong>
            <span>待入库</span>
          </div>
        </button>
        <div class="file-stat-card">
          <FileText :size="16" />
          <div class="file-stat-inline">
            <strong>{{ fileStats.count }}</strong>
            <span>文件</span>
          </div>
        </div>
        <div v-if="fileStats.sizeText" class="file-stat-card">
          <Database :size="16" />
          <div class="file-stat-inline">
            <strong>{{ fileStats.sizeText }}</strong>
            <span>总大小</span>
          </div>
        </div>
        <button
          type="button"
          class="file-stat-card file-stat-repair"
          :disabled="statsRepairing"
          title="修复缺失的 Chunk/Token 统计"
          @click="repairStats"
        >
          <LoaderCircle v-if="statsRepairing" :size="16" class="file-stat-spinner" />
          <Database v-else :size="16" />
          <div class="file-stat-inline">
            <strong>{{ fileStats.chunkText }}</strong>
            <span>Chunks</span>
          </div>
        </button>
        <button
          type="button"
          class="file-stat-card file-stat-repair"
          :disabled="statsRepairing"
          title="修复缺失的 Chunk/Token 统计"
          @click="repairStats"
        >
          <LoaderCircle v-if="statsRepairing" :size="16" class="file-stat-spinner" />
          <Hash v-else :size="16" />
          <div class="file-stat-inline">
            <strong>{{ fileStats.tokenText }}</strong>
            <span>Tokens</span>
          </div>
        </button>
      </div>
    </div>

    <ElBreadcrumb v-if="breadcrumbs.length > 1" class="crumbs">
      <ElBreadcrumbItem v-for="item in breadcrumbs" :key="item.id || 'root'">
        <a href="javascript:;" @click="openFolder(item.id)">{{ item.name }}</a>
      </ElBreadcrumbItem>
    </ElBreadcrumb>

    <ElCard class="art-table-card file-table-card" shadow="never">
      <ArtTable
        :loading="loading"
        :data="documents"
        :columns="columns"
        :pagination="pagination"
        :show-table-header="false"
        :height="480"
        empty-text="暂无文件"
        empty-height="280px"
        @row-click="onRowClick"
        @pagination:size-change="handleSizeChange"
        @pagination:current-change="handleCurrentChange"
      >
        <template #filename="{ row }">
          <div class="file-name-cell">
            <ArtSvgIcon
              :icon="isFolder(row) ? 'ri:folder-2-fill' : 'ri:file-text-line'"
              class="file-name-icon"
              :class="{ 'is-folder': isFolder(row) }"
            />
            <div class="file-name-text">
              <p class="file-name">{{ row.filename || row.name || row.file_id }}</p>
              <p v-if="row.file_id || row.doc_id" class="file-id">
                {{ row.file_id || row.doc_id }}
              </p>
            </div>
          </div>
        </template>

        <template #status="{ row }">
          <ElTag :type="getStatusConfig(row.status).type" effect="light" size="small">
            {{ getStatusConfig(row.status).text }}
          </ElTag>
        </template>

        <template #file_size="{ row }">
          {{ formatSizeCell(row.file_size ?? row.size) }}
        </template>

        <template #operation="{ row }">
          <div class="row-actions" @click.stop>
            <ArtButtonTable
              class="file-row-action"
              type="view"
              title="详情"
              @click="openDetail(row)"
            />
            <ArtButtonTable
              v-if="canShowIndexButton(row)"
              class="file-row-action"
              icon="ri:database-2-line"
              :icon-class="getIndexActionIconClass(row)"
              :title="getIndexActionLabel(row)"
              @click="openIndexOne(row)"
            />
            <ArtButtonTable
              class="file-row-action"
              icon="ri:download-2-line"
              icon-color="var(--el-color-primary)"
              button-bg-color="var(--el-color-primary-light-9)"
              title="下载"
              @click="download(row)"
            />
            <ArtButtonTable
              class="file-row-action"
              type="delete"
              title="删除"
              @click="removeOne(row)"
            />
          </div>
        </template>
      </ArtTable>
    </ElCard>

    <KnowledgeUploadDialog v-model="uploadVisible" :kb-id="kbId" @uploaded="onUploaded" />
    <KnowledgeFileDetailDialog v-model="detailVisible" :kb-id="kbId" :file-id="detailFileId" />
    <KnowledgeIndexConfigDialog
      v-model="indexConfigVisible"
      :title="indexConfigTitle"
      :pending-count="indexPendingCount"
      :default-preset-id="defaultChunkPresetId"
      :initial-params="indexInitialParams"
      :loading="indexSubmitting"
      @confirm="submitIndex"
    />
  </div>
</template>

<script setup lang="ts">
  import { Database, FileText, FolderPlus, Hash, LoaderCircle, Upload } from 'lucide-vue-next'
  import { ElMessage, ElMessageBox } from 'element-plus'
  import type { ColumnOption } from '@/types'
  import { knowledgeApi } from '@/api/knowledge'
  import { useKnowledgeStore } from '@/store/modules/knowledge'
  import { useTaskerStore } from '@/store/modules/tasker'
  import { unwrapApiData } from '@/utils/apiData'
  import { formatFileSize, parseDownloadFilename } from '@/utils/workspace'
  import {
    canShowIndexButton,
    getIndexActionIconClass,
    getIndexActionLabel,
    getIndexConfigTitle
  } from '@/utils/knowledgeFilePolicy'
  import KnowledgeFileDetailDialog from './KnowledgeFileDetailDialog.vue'
  import KnowledgeIndexConfigDialog from './KnowledgeIndexConfigDialog.vue'
  import KnowledgeUploadDialog from './KnowledgeUploadDialog.vue'

  const props = defineProps<{ kbId: string }>()
  const store = useKnowledgeStore()
  const tasker = useTaskerStore()

  const loading = ref(false)
  const actionLoading = ref(false)
  const statsRepairing = ref(false)
  const documents = ref<any[]>([])
  const search = ref('')
  const page = ref(1)
  const pageSize = ref(20)
  const total = ref(0)
  const parentId = ref<string | null>(null)
  const breadcrumbs = ref<Array<{ id: string | null; name: string }>>([
    { id: null, name: '根目录' }
  ])
  const uploadVisible = ref(false)
  const detailVisible = ref(false)
  const detailFileId = ref('')
  const indexConfigVisible = ref(false)
  const indexConfigTitle = ref('入库参数配置')
  const indexPendingCount = ref(0)
  const indexFileIds = ref<string[]>([])
  const indexInitialParams = ref<Record<string, unknown> | null>(null)
  const indexSubmitting = ref(false)

  const pagination = computed(() => ({
    current: page.value,
    size: pageSize.value,
    total: total.value
  }))

  const columns = computed<ColumnOption[]>(() => [
    { type: 'index', width: 60, label: '序号' },
    {
      prop: 'filename',
      label: '名称',
      minWidth: 168,
      useSlot: true
    },
    {
      prop: 'status',
      label: '状态',
      width: 120,
      useSlot: true
    },
    {
      prop: 'file_type',
      label: '类型',
      width: 100,
      formatter: (row) => row.file_type || (isFolder(row) ? '文件夹' : '-')
    },
    {
      prop: 'file_size',
      label: '大小',
      width: 110,
      useSlot: true
    },
    {
      prop: 'chunk_count',
      label: 'Chunk',
      width: 90,
      formatter: (row) => row.chunk_count ?? '-'
    },
    {
      prop: 'operation',
      label: '操作',
      width: 188,
      fixed: 'right',
      useSlot: true
    }
  ])

  const STATUS_CONFIG: Record<
    string,
    { type: 'success' | 'info' | 'warning' | 'danger'; text: string }
  > = {
    uploaded: { type: 'warning', text: '待解析' },
    done: { type: 'success', text: '已入库' },
    completed: { type: 'success', text: '已完成' },
    indexed: { type: 'success', text: '已入库' },
    parsed: { type: 'info', text: '待入库' },
    pending: { type: 'warning', text: '待处理' },
    waiting: { type: 'warning', text: '等待中' },
    processing: { type: 'warning', text: '处理中' },
    parsing: { type: 'warning', text: '解析中' },
    indexing: { type: 'warning', text: '入库中' },
    error_parsing: { type: 'danger', text: '重试解析' },
    error_indexing: { type: 'danger', text: '重试入库' },
    failed: { type: 'danger', text: '入库失败' },
    error: { type: 'danger', text: '失败' }
  }

  const getStatusConfig = (status: unknown) => {
    const key = String(status || '').toLowerCase()
    return STATUS_CONFIG[key] || { type: 'info' as const, text: String(status || '-') }
  }

  const isFolder = (row: any) => !!(row?.is_folder || row?.is_dir || row?.type === 'folder')

  const databaseStats = computed(() => (store.currentDatabase as any)?.stats || {})
  const pendingParseCount = computed(() => Number(databaseStats.value.pending_parse_count || 0))
  const pendingIndexCount = computed(() => Number(databaseStats.value.pending_index_count || 0))

  const defaultChunkPresetId = computed(() => {
    const extra = (store.currentDatabase as any)?.additional_params || {}
    return String(extra.chunk_preset_id || 'general')
  })

  const formatStatNumber = (value: unknown) => {
    const number = Number(value ?? 0)
    return Number.isFinite(number) ? number.toLocaleString('zh-CN') : '0'
  }

  const formatTokenStatNumber = (value: unknown) => {
    const number = Number(value ?? 0)
    if (!Number.isFinite(number)) return '0'
    const absNumber = Math.abs(number)
    if (absNumber > 1024 * 1000) return `${(number / 1_000_000).toFixed(1)} m`
    if (absNumber >= 1000) return `${Math.round(number / 1000).toLocaleString('zh-CN')} k`
    return number.toLocaleString('zh-CN')
  }

  const fileStats = computed(() => {
    const stats = databaseStats.value
    const statsFileCount = Number(stats.file_count)
    const totalSize = Number(stats.total_size || 0)
    return {
      count: Number.isFinite(statsFileCount) ? statsFileCount : 0,
      sizeText: totalSize > 0 ? formatFileSize(totalSize) : '',
      chunkText: formatStatNumber(stats.chunk_count),
      tokenText: formatTokenStatNumber(stats.token_count)
    }
  })

  const formatSizeCell = (size: unknown) => {
    if (size == null || size === '') return '-'
    return formatFileSize(size as number)
  }

  const trackMaybe = async (result: any) => {
    const taskId = result?.task_id || result?.id || result?.data?.task_id
    if (taskId) await tasker.trackTask(String(taskId))
  }

  const refreshStats = async () => {
    if (!props.kbId) return
    await store.loadDatabase(props.kbId)
  }

  const loadDocuments = async () => {
    loading.value = true
    try {
      const result: any = await knowledgeApi.listDocuments(props.kbId, {
        page: page.value,
        page_size: pageSize.value,
        keyword: search.value || undefined,
        parent_id: parentId.value || undefined
      })
      const data: any = unwrapApiData(result, result) || {}
      const items = Array.isArray(data)
        ? data
        : data?.items || data?.entries || data?.documents || data?.files || []
      documents.value = Array.isArray(items) ? items : []
      total.value = Number(data?.total ?? data?.pagination?.total_items ?? documents.value.length)
      if (Array.isArray(data?.breadcrumbs)) breadcrumbs.value = data.breadcrumbs
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载文档失败')
    } finally {
      loading.value = false
    }
  }

  const refreshAll = async () => {
    await Promise.all([loadDocuments(), refreshStats()])
  }

  const onUploaded = async () => {
    await refreshAll()
  }

  const handleSizeChange = (size: number) => {
    pageSize.value = size
    page.value = 1
    loadDocuments()
  }

  const handleCurrentChange = (current: number) => {
    page.value = current
    loadDocuments()
  }

  const onRowClick = (row: any) => {
    if (isFolder(row)) {
      openFolder(row.file_id || row.id)
    }
  }

  const openFolder = (id: string | null) => {
    parentId.value = id
    page.value = 1
    loadDocuments()
  }

  const createFolder = async () => {
    try {
      const { value } = await ElMessageBox.prompt('请输入文件夹名称', '新建文件夹')
      if (!value?.trim()) return
      await knowledgeApi.createFolder(props.kbId, value.trim(), parentId.value)
      ElMessage.success('已创建')
      await refreshAll()
    } catch {
      // 取消
    }
  }

  const openPendingIndex = () => {
    const count = pendingIndexCount.value
    if (count <= 0) {
      ElMessage.info('没有待入库文档')
      return
    }
    indexFileIds.value = []
    indexPendingCount.value = count
    indexInitialParams.value = null
    indexConfigTitle.value = '待入库文件参数配置'
    indexConfigVisible.value = true
  }

  const loadRecordProcessingParams = async (row: any) => {
    if (row?.processing_params) return row.processing_params
    const fileId = String(row.file_id || row.doc_id || row.id)
    const detail: any = await knowledgeApi.getDocumentInfo(props.kbId, fileId)
    return detail?.processing_params || detail?.data?.processing_params || null
  }

  const openIndexOne = async (row: any) => {
    if (!canShowIndexButton(row)) return
    indexFileIds.value = [String(row.file_id || row.doc_id || row.id)]
    indexPendingCount.value = 0
    indexConfigTitle.value = getIndexConfigTitle(row)
    try {
      indexInitialParams.value = await loadRecordProcessingParams(row)
    } catch {
      indexInitialParams.value = null
    }
    indexConfigVisible.value = true
  }

  const submitIndex = async (params: Record<string, unknown>) => {
    indexSubmitting.value = true
    try {
      const result: any = indexPendingCount.value
        ? await knowledgeApi.indexPendingDocuments(props.kbId, params)
        : await knowledgeApi.indexDocuments(props.kbId, indexFileIds.value, params)
      await trackMaybe(result)
      ElMessage.success(indexPendingCount.value ? '已提交待入库任务' : '已提交入库任务')
      indexConfigVisible.value = false
      indexFileIds.value = []
      indexPendingCount.value = 0
      await refreshAll()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '入库失败')
    } finally {
      indexSubmitting.value = false
    }
  }

  const confirmBatchParse = async () => {
    const count = pendingParseCount.value
    if (count <= 0) {
      ElMessage.info('没有待解析文档')
      return
    }
    try {
      await ElMessageBox.confirm(
        `将提交 ${formatStatNumber(count)} 个待解析文件，任务会在后台按批处理，可在任务中心查看进度。`,
        '解析待解析文件',
        { confirmButtonText: '提交解析', type: 'warning' }
      )
      actionLoading.value = true
      const result = await knowledgeApi.parsePendingDocuments(props.kbId)
      await trackMaybe(result)
      ElMessage.success('已提交待解析任务')
      await refreshAll()
    } catch {
      // 取消
    } finally {
      actionLoading.value = false
    }
  }

  const repairStats = async () => {
    if (statsRepairing.value) return
    statsRepairing.value = true
    try {
      const result: any = await knowledgeApi.repairDatabaseStats(props.kbId)
      await refreshStats()
      const updatedTokenFiles = Number(result?.updated_token_files || 0)
      const updatedChunkFiles = Number(result?.updated_chunk_files || 0)
      if (updatedTokenFiles || updatedChunkFiles) {
        ElMessage.success(
          `已修复 ${updatedTokenFiles} 个 Token 统计，${updatedChunkFiles} 个 Chunk 统计`
        )
      } else {
        ElMessage.info('统计已是最新')
      }
    } catch (error) {
      ElMessage.error((error as Error)?.message || '统计修复失败')
    } finally {
      statsRepairing.value = false
    }
  }

  const openDetail = (row: any) => {
    detailFileId.value = row.file_id || row.doc_id || row.id
    detailVisible.value = true
  }

  const download = async (row: any) => {
    const fileId = row.file_id || row.doc_id || row.id
    try {
      const response = await knowledgeApi.downloadDocument(props.kbId, fileId)
      const blob = await response.blob()
      const filename =
        parseDownloadFilename(
          response.headers.get('Content-Disposition') ||
            response.headers.get('content-disposition') ||
            ''
        ) ||
        row.filename ||
        fileId
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = filename
      link.click()
      URL.revokeObjectURL(url)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '下载失败')
    }
  }

  const removeOne = async (row: any) => {
    const fileId = row.file_id || row.doc_id || row.id
    try {
      await ElMessageBox.confirm('确认删除该文件？', '删除', { type: 'warning' })
      await knowledgeApi.deleteDocument(props.kbId, fileId)
      ElMessage.success('已删除')
      await refreshAll()
    } catch {
      // 取消
    }
  }

  watch(
    () => props.kbId,
    async () => {
      parentId.value = null
      page.value = 1
      await refreshAll()
    },
    { immediate: true }
  )

  watch(
    () =>
      Object.values(tasker.tasks)
        .map((t) => `${t.id}:${t.status}`)
        .join('|'),
    () => {
      const hasDone = Object.values(tasker.tasks).some((t) => tasker.isTerminal(t.status))
      if (hasDone) refreshAll()
    }
  )
</script>

<style scoped>
  .file-management-info {
    display: flex;
    flex-wrap: nowrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 12px;
  }

  .file-toolbar-left {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: 1;
    min-width: 0;
  }

  .file-search {
    width: 220px;
    max-width: 100%;
  }

  .file-search :deep(.el-input__wrapper) {
    min-height: 36px;
    height: 36px;
    box-sizing: border-box;
  }

  .panel-action {
    min-height: 36px;
    height: 36px;
    padding: 0 12px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    border-radius: 8px;
    border: 1px solid transparent;
    font: inherit;
    font-weight: 500;
    font-size: 13px;
    cursor: pointer;
    appearance: none;
    white-space: nowrap;
    flex-shrink: 0;
    box-sizing: border-box;
    transition:
      background-color 0.15s ease,
      border-color 0.15s ease,
      color 0.15s ease;
  }

  .panel-action-primary {
    background: var(--el-color-primary);
    color: var(--el-color-white);
  }

  .panel-action-primary:hover {
    background: var(--el-color-primary-light-3);
  }

  .panel-action-secondary {
    background: var(--el-bg-color);
    border-color: var(--el-border-color);
    color: var(--el-text-color-regular);
  }

  .panel-action-secondary:hover {
    border-color: var(--el-border-color-darker);
    color: var(--el-text-color-primary);
  }

  .file-panel-status {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    flex-shrink: 0;
  }

  .file-stat-card {
    min-width: 87px;
    min-height: 36px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 10px;
    border-radius: 8px;
    background: var(--el-color-primary-light-9);
    border: 1px solid var(--el-border-color-lighter);
    color: var(--el-color-primary);
    font: inherit;
    appearance: none;
    text-align: left;
    white-space: nowrap;
  }

  .file-stat-inline {
    display: flex;
    flex-direction: row;
    align-items: baseline;
    gap: 4px;
  }

  .file-stat-inline strong {
    font-size: 14px;
    line-height: 1.2;
    color: var(--el-text-color-primary);
    white-space: nowrap;
  }

  .file-stat-inline span {
    font-size: 11px;
    color: var(--el-text-color-secondary);
    white-space: nowrap;
  }

  .file-stat-action {
    cursor: pointer;
  }

  .file-stat-parse {
    color: var(--el-color-warning);
    border-color: var(--el-color-warning-light-7);
    background: var(--el-color-warning-light-9);
  }

  .file-stat-parse:hover:not(:disabled) {
    border-color: var(--el-color-warning);
  }

  .file-stat-index {
    color: var(--el-color-primary);
    border-color: var(--el-color-primary-light-7);
    background: var(--el-color-primary-light-9);
  }

  .file-stat-index:hover:not(:disabled) {
    border-color: var(--el-color-primary);
  }

  .file-stat-action:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }

  .file-stat-repair {
    cursor: pointer;
  }

  .file-stat-repair:hover:not(:disabled) {
    border-color: var(--el-color-primary-light-5);
    background: var(--el-color-primary-light-8);
  }

  .file-stat-repair:disabled {
    cursor: wait;
    opacity: 0.72;
  }

  .file-stat-spinner {
    animation: file-stat-spin 0.8s linear infinite;
  }

  @keyframes file-stat-spin {
    to {
      transform: rotate(360deg);
    }
  }

  .crumbs {
    margin-bottom: 10px;
  }

  .file-table-card {
    margin-top: 0;
    flex: none;
  }

  .file-table-card :deep(.el-card__body) {
    height: auto;
    overflow: visible;
    padding: 12px 16px 16px;
  }

  .file-name-cell {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }

  .file-name-icon {
    flex-shrink: 0;
    font-size: 18px;
    color: var(--el-text-color-secondary);
  }

  .file-name-icon.is-folder {
    color: var(--el-color-warning);
  }

  .file-name-text {
    min-width: 0;
  }

  .file-name {
    margin: 0;
    overflow: hidden;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-id {
    margin: 2px 0 0;
    overflow: hidden;
    font-size: 12px;
    color: var(--el-text-color-secondary);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-actions {
    display: flex;
    flex-wrap: nowrap;
    gap: 6px;
    align-items: center;
  }

  .row-actions :deep(.file-row-action) {
    width: 32px;
    min-width: 32px;
    flex: 0 0 32px;
    height: 32px;
    padding: 0;
    margin-right: 0;
    font-size: 15px;
  }
</style>
