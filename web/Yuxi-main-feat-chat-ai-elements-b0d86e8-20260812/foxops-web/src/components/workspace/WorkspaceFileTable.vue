<template>
  <section class="workspace-file-table">
    <div class="file-toolbar">
      <ElBreadcrumb separator="/">
        <ElBreadcrumbItem
          v-for="(item, index) in resolvedBreadcrumbs"
          :key="`${item.path}-${index}`"
        >
          <button type="button" class="breadcrumb-button" @click="$emit('breadcrumb', item, index)">
            {{ item.name }}
          </button>
        </ElBreadcrumbItem>
      </ElBreadcrumb>
      <div class="toolbar-actions">
        <span>{{ pagination ? `${pagination.total} 项` : `${entries.length} 项` }}</span>
        <ElButton
          v-if="!readonly"
          size="small"
          :type="selectionMode ? 'primary' : 'default'"
          @click="$emit('update:selection-mode', !selectionMode)"
        >
          多选
        </ElButton>
        <ElButton
          v-if="selectionMode && !readonly"
          size="small"
          type="danger"
          :disabled="!selectedPaths.length"
          :loading="deletingPaths.length > 0"
          @click="$emit('delete-selected')"
        >
          删除选中
        </ElButton>
      </div>
    </div>

    <ElTable
      ref="tableRef"
      v-loading="loading"
      :data="entries"
      height="100%"
      row-key="path"
      highlight-current-row
      :current-row-key="selectedPath"
      :row-class-name="rowClassName"
      @row-click="(row) => $emit('open', row)"
      @selection-change="handleSelectionChange"
    >
      <ElTableColumn
        v-if="selectionMode && !readonly"
        type="selection"
        width="44"
        :selectable="(row) => !isDeleting(row.path)"
      />
      <ElTableColumn label="名称" min-width="220">
        <template #default="{ row }">
          <div class="name-cell">
            <FileTypeIcon :name="row.name || row.path" :is-dir="row.is_dir" />
            <span :title="row.name">{{ row.name }}</span>
          </div>
        </template>
      </ElTableColumn>
      <ElTableColumn label="大小" width="100">
        <template #default="{ row }">{{ row.is_dir ? '-' : formatFileSize(row.size) }}</template>
      </ElTableColumn>
      <ElTableColumn label="修改时间" width="170">
        <template #default="{ row }">{{ formatModified(row.modified_at) }}</template>
      </ElTableColumn>
      <ElTableColumn label="操作" width="70" align="center">
        <template #default="{ row }">
          <ElDropdown
            v-if="!row.is_dir || !readonly"
            trigger="click"
            @command="(command) => handleCommand(command, row)"
          >
            <button type="button" class="more-button" :disabled="isDeleting(row.path)" @click.stop>
              <ArtSvgIcon icon="ri:more-2-fill" />
            </button>
            <template #dropdown>
              <ElDropdownMenu>
                <ElDropdownItem v-if="!row.is_dir" command="download">
                  <ArtSvgIcon icon="ri:download-line" />
                  下载
                </ElDropdownItem>
                <ElDropdownItem v-if="!readonly" command="delete" divided>
                  <span class="danger-action">
                    <ArtSvgIcon icon="ri:delete-bin-line" />
                    删除
                  </span>
                </ElDropdownItem>
              </ElDropdownMenu>
            </template>
          </ElDropdown>
        </template>
      </ElTableColumn>
      <template #empty>
        <ElEmpty description="当前文件夹为空" />
      </template>
    </ElTable>

    <div v-if="pagination" class="file-pagination">
      <ElPagination
        :current-page="pagination.page"
        :page-size="pagination.pageSize"
        :page-sizes="[100, 300, 500]"
        :total="pagination.total"
        layout="total, sizes, prev, pager, next"
        @current-change="(page) => $emit('page-change', page, pagination?.pageSize || 100)"
        @size-change="(pageSize) => $emit('page-change', 1, pageSize)"
      />
    </div>
  </section>
</template>

<script setup lang="ts">
  import type { WorkspaceEntry } from '@/api/workspace'
  import { buildBreadcrumbs, formatFileSize } from '@/utils/workspace'
  import FileTypeIcon from './FileTypeIcon.vue'

  interface Breadcrumb {
    name: string
    path: string
    parentId?: string | null
    pathPrefix?: string
  }

  interface Pagination {
    page: number
    pageSize: number
    total: number
  }

  const props = withDefaults(
    defineProps<{
      entries?: WorkspaceEntry[]
      breadcrumbs?: Breadcrumb[] | null
      currentPath?: string
      rootLabel?: string
      selectedPath?: string
      selectedPaths?: string[]
      deletingPaths?: string[]
      selectionMode?: boolean
      loading?: boolean
      readonly?: boolean
      pagination?: Pagination | null
    }>(),
    {
      entries: () => [],
      breadcrumbs: null,
      currentPath: '/',
      rootLabel: '工作区',
      selectedPath: '',
      selectedPaths: () => [],
      deletingPaths: () => [],
      selectionMode: false,
      loading: false,
      readonly: false,
      pagination: null
    }
  )

  const emit = defineEmits<{
    open: [entry: WorkspaceEntry]
    breadcrumb: [item: Breadcrumb, index: number]
    'update:selected-paths': [paths: string[]]
    'update:selection-mode': [enabled: boolean]
    'delete-selected': []
    delete: [entry: WorkspaceEntry]
    download: [entry: WorkspaceEntry]
    'page-change': [page: number, pageSize: number]
  }>()

  const resolvedBreadcrumbs = computed(() => {
    if (props.breadcrumbs?.length) return props.breadcrumbs
    return buildBreadcrumbs(props.currentPath, props.rootLabel)
  })

  const deletingPathSet = computed(() => new Set(props.deletingPaths))
  const isDeleting = (path: string) => deletingPathSet.value.has(path)
  const rowClassName = ({ row }: { row: WorkspaceEntry }) =>
    isDeleting(row.path) ? 'is-deleting' : ''

  const handleSelectionChange = (rows: WorkspaceEntry[]) =>
    emit(
      'update:selected-paths',
      rows.map((row) => row.path)
    )

  const handleCommand = (command: string, row: WorkspaceEntry) =>
    command === 'download' ? emit('download', row) : emit('delete', row)

  const formatModified = (value?: string | null) => {
    if (!value) return '-'
    const date = new Date(/[zZ]$|[+-]\d{2}:\d{2}$/.test(value) ? value : `${value}Z`)
    return Number.isNaN(date.getTime()) ? '-' : date.toLocaleString('zh-CN', { hour12: false })
  }
</script>

<style scoped>
  .workspace-file-table {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    height: 100%;
  }

  .file-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    min-height: 52px;
    padding: 0 16px;
    border-bottom: 1px solid var(--el-border-color-lighter);
  }

  .breadcrumb-button {
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    cursor: pointer;
  }

  .toolbar-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--el-text-color-secondary);
    font-size: 12px;
  }

  .name-cell {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    cursor: pointer;
  }

  .name-cell span:last-child {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .more-button {
    display: inline-flex;
    padding: 5px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--el-text-color-secondary);
    cursor: pointer;
  }

  .more-button:hover:not(:disabled) {
    background: var(--el-fill-color);
  }

  .more-button:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .danger-action {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--el-color-danger);
  }

  .file-pagination {
    display: flex;
    justify-content: flex-end;
    padding: 10px 16px;
    border-top: 1px solid var(--el-border-color-lighter);
  }

  :deep(.is-deleting) {
    opacity: 0.55;
  }
</style>
