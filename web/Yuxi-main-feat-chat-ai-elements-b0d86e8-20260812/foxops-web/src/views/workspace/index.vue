<template>
  <div class="page-content workspace-page !p-0" :style="{ height: containerMinHeight }">
    <header class="workspace-header">
      <div class="header-title">
        <h1>工作区</h1>
        <ElTag v-if="loadingTree || loadingPreview" size="small" type="info" effect="plain">
          加载中
        </ElTag>
      </div>
      <div class="header-actions">
        <ElButton :disabled="!isPersonalSource" @click="openCreateDirectoryModal">
          新建文件夹
        </ElButton>
        <ElButton
          type="primary"
          :loading="uploadingFile"
          :disabled="!isPersonalSource"
          @click="openUploadFilePicker"
        >
          上传文件
        </ElButton>
      </div>
    </header>

    <input
      ref="uploadInputRef"
      class="upload-input"
      type="file"
      multiple
      @change="handleUploadInputChange"
    />

    <div class="workspace-shell" :class="{ 'is-sidebar-collapsed': sidebarCollapsed }">
      <div v-if="!sidebarCollapsed" class="workspace-sidebar-slot">
        <button
          type="button"
          class="sidebar-collapse-action"
          aria-label="收起工作区侧边栏"
          @click="sidebarCollapsed = true"
        >
          <ArtSvgIcon icon="ri:arrow-left-s-line" />
        </button>
        <WorkspaceSidebar
          :active-key="activeSourceKey"
          :current-path="currentPath"
          :databases="databases"
          :loading-databases="loadingDatabases"
          :database-error="databaseLoadError"
          :current-uid="currentUid"
          @select-personal="selectPersonalWorkspace"
          @select-database="selectDatabase"
          @select-path="selectWorkspacePath"
          @retry-databases="loadDatabases"
        />
      </div>
      <button
        v-else
        type="button"
        class="sidebar-expand-action"
        aria-label="展开工作区侧边栏"
        @click="sidebarCollapsed = false"
      >
        <ArtSvgIcon icon="ri:arrow-right-s-line" />
      </button>

      <main class="workspace-main">
        <template v-if="activeSourceKey === 'personal' || selectedDatabase">
          <WorkspaceFileTable
            :entries="entries"
            :current-path="currentPath"
            :selected-path="selectedEntry?.path || ''"
            :selected-paths="selectedPaths"
            :deleting-paths="deletingPaths"
            :selection-mode="selectionMode"
            :loading="loadingTree"
            :readonly="isKnowledgeSource"
            :root-label="selectedDatabase?.name || '工作区'"
            :breadcrumbs="isKnowledgeSource ? knowledgeBreadcrumbItems : null"
            :pagination="isKnowledgeSource ? knowledgePagination : null"
            @open="handleSelectEntry"
            @breadcrumb="handleListBreadcrumbClick"
            @update:selected-paths="selectedPaths = $event"
            @update:selection-mode="handleSelectionModeChange"
            @delete-selected="confirmDeleteEntries(selectedEntries)"
            @delete="(entry) => confirmDeleteEntries([entry])"
            @download="downloadEntry"
            @page-change="handleKnowledgePageChange"
          />
        </template>

        <div v-else class="workspace-placeholder">
          <ArtSvgIcon icon="ri:database-2-line" class="placeholder-icon" />
          <h2>知识库</h2>
          <p>请选择一个可访问知识库以浏览文件。</p>
        </div>
      </main>
    </div>

    <ElDialog
      v-model="createDirectoryModalVisible"
      title="新建文件夹"
      width="420px"
      :close-on-click-modal="!creatingDirectory"
      @closed="newDirectoryName = ''"
    >
      <ElInput
        v-model="newDirectoryName"
        placeholder="请输入文件夹名称"
        :disabled="creatingDirectory"
        @keyup.enter="createDirectory"
      />
      <template #footer>
        <ElButton :disabled="creatingDirectory" @click="createDirectoryModalVisible = false">
          取消
        </ElButton>
        <ElButton type="primary" :loading="creatingDirectory" @click="createDirectory">
          创建
        </ElButton>
      </template>
    </ElDialog>

    <ElDialog
      v-model="previewModalVisible"
      width="92vw"
      top="4vh"
      :show-close="false"
      :close-on-click-modal="false"
      :close-on-press-escape="true"
      :destroy-on-close="true"
      append-to-body
      class="workspace-file-preview-modal"
      @closed="handlePreviewModalClosed"
    >
      <WorkspaceFilePreview
        v-if="previewModalVisible && previewFile"
        :file="previewFile"
        :file-path="selectedPreviewPath"
        :editable="isPersonalSource"
        :saving="savingPreviewFile"
        @close="closePreview"
        @save="handleSavePreviewFile"
        @download="downloadSelectedPreview"
      />
    </ElDialog>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { knowledgeApi, type AccessibleKnowledgeBase } from '@/api/knowledge'
  import { workspaceApi, type WorkspaceEntry } from '@/api/workspace'
  import WorkspaceFilePreview, {
    type PreviewFile
  } from '@/components/workspace/WorkspaceFilePreview.vue'
  import WorkspaceFileTable from '@/components/workspace/WorkspaceFileTable.vue'
  import WorkspaceSidebar from '@/components/workspace/WorkspaceSidebar.vue'
  import { useAutoLayoutHeight } from '@/hooks/core/useLayoutHeight'
  import { useUserStore } from '@/store/modules/user'
  import {
    getPreviewType,
    normalizePreviewResponse,
    parseDownloadFilename
  } from '@/utils/workspace'

  defineOptions({ name: 'Workspace' })

  const MAX_UPLOAD_FILES = 50

  interface KnowledgeBreadcrumb {
    name: string
    path: string
    parentId?: string | null
    pathPrefix?: string
    isVirtualFolder?: boolean
  }

  const userStore = useUserStore()
  const { containerMinHeight } = useAutoLayoutHeight()

  const activeSourceKey = ref('personal')
  const currentPath = ref('/')
  const knowledgeBreadcrumbItems = ref<KnowledgeBreadcrumb[]>([])
  const entries = ref<WorkspaceEntry[]>([])
  const selectedEntry = ref<WorkspaceEntry | null>(null)
  const selectedPaths = ref<string[]>([])
  const selectionMode = ref(false)
  const previewFile = ref<PreviewFile | null>(null)
  const previewObjectUrl = ref('')
  const previewModalVisible = ref(false)
  const loadingTree = ref(false)
  const loadingPreview = ref(false)
  const savingPreviewFile = ref(false)
  const loadingDatabases = ref(false)
  const databaseLoadError = ref('')
  const databases = ref<AccessibleKnowledgeBase[]>([])
  const selectedDatabase = ref<AccessibleKnowledgeBase | null>(null)
  const createDirectoryModalVisible = ref(false)
  const newDirectoryName = ref('')
  const creatingDirectory = ref(false)
  const uploadingFile = ref(false)
  const uploadInputRef = ref<HTMLInputElement | null>(null)
  const deletingPaths = ref<string[]>([])
  const sidebarCollapsed = ref(false)
  const previewRequestId = ref(0)

  const knowledgeFileBrowser = reactive({
    parentId: null as string | null,
    pathPrefix: '',
    page: 1,
    pageSize: 100,
    total: 0,
    hasMore: false
  })

  const currentUid = computed(() => String(userStore.info?.uid || ''))
  const isPersonalSource = computed(() => activeSourceKey.value === 'personal')
  const isKnowledgeSource = computed(() => activeSourceKey.value.startsWith('database:'))
  const selectedPreviewPath = computed(() =>
    selectedEntry.value?.source === 'knowledge'
      ? selectedEntry.value.name || ''
      : selectedEntry.value?.path || ''
  )
  const knowledgePagination = computed(() => ({
    page: knowledgeFileBrowser.page,
    pageSize: knowledgeFileBrowser.pageSize,
    total: knowledgeFileBrowser.total
  }))
  const selectedEntries = computed(() => {
    const selectedPathSet = new Set(selectedPaths.value)
    return entries.value.filter((entry) => selectedPathSet.has(entry.path))
  })

  const revokePreviewObjectUrl = () => {
    if (!previewObjectUrl.value) return
    window.URL.revokeObjectURL(previewObjectUrl.value)
    previewObjectUrl.value = ''
  }

  const buildPreviewLoadingFile = (
    entry: WorkspaceEntry,
    baseFile: WorkspaceEntry = entry
  ): PreviewFile => ({
    ...baseFile,
    ...entry,
    name: entry.name,
    content: 'Loading...',
    supported: true,
    previewType: 'text',
    message: '',
    previewUrl: ''
  })

  const buildPreviewErrorFile = (entry: WorkspaceEntry, error: unknown): PreviewFile => ({
    ...entry,
    name: entry.name,
    content: `Error loading file: ${(error as Error)?.message || 'unknown error'}`,
    supported: false,
    previewType: 'unsupported',
    message: (error as Error)?.message || '文件预览失败',
    previewUrl: ''
  })

  const startPreviewRequest = (entry: WorkspaceEntry, baseFile: WorkspaceEntry = entry) => {
    const requestId = previewRequestId.value + 1
    previewRequestId.value = requestId
    selectedEntry.value = entry
    revokePreviewObjectUrl()
    previewFile.value = buildPreviewLoadingFile(entry, baseFile)
    previewModalVisible.value = true
    loadingPreview.value = true
    return requestId
  }

  const isCurrentPreviewEntry = (requestId: number, entry: WorkspaceEntry) => {
    if (previewRequestId.value !== requestId) return false
    if (entry.source === 'knowledge') {
      return selectedEntry.value?.file_id === entry.file_id
    }
    return selectedEntry.value?.path === entry.path
  }

  const applyPreviewFile = (requestId: number, entry: WorkspaceEntry, file: PreviewFile) => {
    if (!isCurrentPreviewEntry(requestId, entry)) {
      if (file.previewUrl) window.URL.revokeObjectURL(file.previewUrl)
      return
    }
    if (file.previewUrl) {
      revokePreviewObjectUrl()
      previewObjectUrl.value = file.previewUrl
    }
    previewFile.value = file
  }

  const showPreviewError = (
    requestId: number,
    entry: WorkspaceEntry,
    error: unknown,
    logMessage: string,
    userMessage: string
  ) => {
    if (!isCurrentPreviewEntry(requestId, entry)) return
    console.warn(logMessage, error)
    previewFile.value = buildPreviewErrorFile(entry, error)
    ElMessage.error(userMessage)
  }

  const finishPreviewRequest = (requestId: number) => {
    if (previewRequestId.value === requestId) loadingPreview.value = false
  }

  const loadWorkspacePreview = async (entry: WorkspaceEntry) => {
    const requestId = startPreviewRequest(entry)
    try {
      const response = await workspaceApi.getFile(entry.path)
      if (!isCurrentPreviewEntry(requestId, entry)) return
      const file = (await normalizePreviewResponse(response, entry)) as PreviewFile
      applyPreviewFile(requestId, entry, file)
    } catch (error) {
      showPreviewError(requestId, entry, error, '加载文件预览失败:', '加载文件预览失败')
    } finally {
      finishPreviewRequest(requestId)
    }
  }

  const loadKnowledgePreview = async (entry: WorkspaceEntry) => {
    const requestId = startPreviewRequest(entry)
    try {
      const previewType = getPreviewType(entry.name)
      const requiresOriginal = ['image', 'pdf', 'spreadsheet', 'office'].includes(previewType)
      const response = requiresOriginal
        ? await workspaceApi.downloadKnowledgeFile(String(entry.kb_id), String(entry.file_id))
        : await workspaceApi.getKnowledgeFile(String(entry.kb_id), String(entry.file_id), 'parsed')
      if (!isCurrentPreviewEntry(requestId, entry)) return
      let file = (await normalizePreviewResponse(response, entry)) as PreviewFile

      // 尚未解析的文本文件仍可退回原文预览，避免只有“无解析结果”的空状态。
      if (
        !requiresOriginal &&
        file.supported === false &&
        ['markdown', 'text', 'html'].includes(previewType)
      ) {
        const originalResponse = await workspaceApi.downloadKnowledgeFile(
          String(entry.kb_id),
          String(entry.file_id)
        )
        if (!isCurrentPreviewEntry(requestId, entry)) return
        file = (await normalizePreviewResponse(originalResponse, entry)) as PreviewFile
      }
      applyPreviewFile(requestId, entry, file)
    } catch (error) {
      showPreviewError(requestId, entry, error, '加载知识库文件预览失败:', '加载知识库文件预览失败')
    } finally {
      finishPreviewRequest(requestId)
    }
  }

  const syncSelectedPaths = () => {
    const entryPathSet = new Set(entries.value.map((entry) => entry.path))
    selectedPaths.value = selectedPaths.value.filter((path) => entryPathSet.has(path))
  }

  const clearWorkspaceSelection = () => {
    selectedPaths.value = []
  }

  const handleSelectionModeChange = (enabled: boolean) => {
    selectionMode.value = enabled
    if (!enabled) clearWorkspaceSelection()
  }

  const loadWorkspaceEntries = async (path = '/') => {
    loadingTree.value = true
    try {
      const response = await workspaceApi.getTree(path)
      entries.value = response.entries || []
      currentPath.value = path
      knowledgeBreadcrumbItems.value = []
      syncSelectedPaths()
      if (!selectedPaths.value.length) selectionMode.value = false
    } catch (error) {
      console.warn('加载工作区目录失败:', error)
      ElMessage.error('加载工作区目录失败')
    } finally {
      loadingTree.value = false
    }
  }

  const loadKnowledgeEntries = async (
    database: AccessibleKnowledgeBase,
    {
      parentId = null as string | null,
      pathPrefix = '',
      page = 1,
      pageSize = knowledgeFileBrowser.pageSize,
      breadcrumbs = null as KnowledgeBreadcrumb[] | null
    } = {}
  ) => {
    if (!database?.kb_id) return

    loadingTree.value = true
    try {
      const response = await workspaceApi.getKnowledgeTree(database.kb_id, {
        parentId,
        pathPrefix,
        page,
        pageSize
      })
      entries.value = response.entries || []
      knowledgeBreadcrumbItems.value = breadcrumbs || [
        {
          name: database.name || '知识库',
          path: '/',
          parentId: null,
          pathPrefix: '',
          isVirtualFolder: false
        }
      ]
      currentPath.value = knowledgeBreadcrumbItems.value.at(-1)?.path || '/'
      Object.assign(knowledgeFileBrowser, {
        parentId: response.parent_id || parentId || null,
        pathPrefix: response.path_prefix || pathPrefix || '',
        page: response.page || page,
        pageSize: response.page_size || pageSize,
        total: response.total || 0,
        hasMore: Boolean(response.has_more)
      })
      syncSelectedPaths()
      if (!selectedPaths.value.length) selectionMode.value = false
    } catch (error) {
      console.warn('加载知识库目录失败:', error)
      entries.value = []
      ElMessage.error((error as Error)?.message || '加载知识库目录失败')
    } finally {
      loadingTree.value = false
    }
  }

  const loadDatabases = async () => {
    loadingDatabases.value = true
    databaseLoadError.value = ''
    try {
      const response = await knowledgeApi.getAccessibleDatabases()
      databases.value = (response?.databases || []).filter(
        (database) => database?.supports_documents !== false
      )
    } catch (error) {
      console.warn('加载可访问知识库失败:', error)
      databases.value = []
      databaseLoadError.value = (error as Error)?.message || '知识库服务连接失败'
      ElMessage.error('知识库列表加载失败，请稍后重试')
    } finally {
      loadingDatabases.value = false
    }
  }

  const selectPersonalWorkspace = async () => {
    const wasKnowledgeSource = isKnowledgeSource.value
    activeSourceKey.value = 'personal'
    selectedDatabase.value = null
    knowledgeBreadcrumbItems.value = []
    closePreview()
    clearWorkspaceSelection()
    if (wasKnowledgeSource || currentPath.value !== '/' || !entries.value.length) {
      await loadWorkspaceEntries('/')
    }
  }

  const selectWorkspacePath = async (path: string) => {
    activeSourceKey.value = 'personal'
    selectedDatabase.value = null
    knowledgeBreadcrumbItems.value = []
    closePreview()
    clearWorkspaceSelection()
    await loadWorkspaceEntries(path)
  }

  const selectKnowledgeBreadcrumb = async (item: KnowledgeBreadcrumb, index: number) => {
    if (!selectedDatabase.value || !item) return
    closePreview()
    clearWorkspaceSelection()
    const breadcrumbs = knowledgeBreadcrumbItems.value.slice(0, index + 1)
    await loadKnowledgeEntries(selectedDatabase.value, {
      parentId: item.parentId || null,
      pathPrefix: item.pathPrefix || '',
      page: 1,
      breadcrumbs
    })
  }

  const handleListBreadcrumbClick = async (item: KnowledgeBreadcrumb, index: number) => {
    if (isKnowledgeSource.value) {
      await selectKnowledgeBreadcrumb(item, index)
      return
    }
    await selectWorkspacePath(item?.path || '/')
  }

  const selectDatabase = async (database: AccessibleKnowledgeBase) => {
    if (database?.supports_documents === false) return
    closePreview()
    clearWorkspaceSelection()
    selectedDatabase.value = database
    activeSourceKey.value = `database:${database.kb_id}`
    await loadKnowledgeEntries(database)
  }

  const openKnowledgeDirectory = async (entry: WorkspaceEntry) => {
    closePreview()
    clearWorkspaceSelection()
    const parentPath = knowledgeBreadcrumbItems.value.at(-1)?.path || '/'
    const nextPath = parentPath === '/' ? `/${entry.name}` : `${parentPath}/${entry.name}`
    const currentBreadcrumb = knowledgeBreadcrumbItems.value.at(-1)
    const isVirtualFolder = Boolean(entry.is_virtual_folder)
    const nextBreadcrumb: KnowledgeBreadcrumb = {
      name: entry.name,
      path: nextPath,
      parentId: isVirtualFolder ? currentBreadcrumb?.parentId || null : entry.file_id,
      pathPrefix: isVirtualFolder ? entry.path_prefix || '' : '',
      isVirtualFolder
    }
    await loadKnowledgeEntries(selectedDatabase.value!, {
      parentId: nextBreadcrumb.parentId || null,
      pathPrefix: nextBreadcrumb.pathPrefix || '',
      page: 1,
      breadcrumbs: [...knowledgeBreadcrumbItems.value, nextBreadcrumb]
    })
  }

  const handleKnowledgePageChange = async (page: number, pageSize: number) => {
    if (!selectedDatabase.value || !isKnowledgeSource.value) return
    closePreview()
    clearWorkspaceSelection()
    const currentBreadcrumb = knowledgeBreadcrumbItems.value.at(-1)
    await loadKnowledgeEntries(selectedDatabase.value, {
      parentId: currentBreadcrumb?.parentId || null,
      pathPrefix: currentBreadcrumb?.pathPrefix || '',
      page,
      pageSize,
      breadcrumbs: [...knowledgeBreadcrumbItems.value]
    })
  }

  const openWorkspaceDirectory = async (entry: WorkspaceEntry) => {
    closePreview()
    clearWorkspaceSelection()
    await loadWorkspaceEntries(entry.path)
  }

  const handleSelectEntry = async (entry: WorkspaceEntry) => {
    if (entry.is_dir) {
      if (isKnowledgeSource.value) {
        await openKnowledgeDirectory(entry)
        return
      }
      await openWorkspaceDirectory(entry)
      return
    }

    if (isKnowledgeSource.value) {
      await loadKnowledgePreview(entry)
      return
    }
    await loadWorkspacePreview(entry)
  }

  const closePreview = () => {
    previewRequestId.value += 1
    previewModalVisible.value = false
    selectedEntry.value = null
    previewFile.value = null
    loadingPreview.value = false
    revokePreviewObjectUrl()
  }

  /** 弹窗被 ESC 等方式关闭时，同步清理预览资源 */
  const handlePreviewModalClosed = () => {
    if (!previewModalVisible.value) {
      selectedEntry.value = null
      previewFile.value = null
      loadingPreview.value = false
      revokePreviewObjectUrl()
    }
  }

  const handleSavePreviewFile = async (content: string) => {
    if (selectedEntry.value?.source === 'knowledge') {
      ElMessage.warning('知识库文件为只读，无法保存')
      return
    }
    if (!selectedEntry.value?.path || savingPreviewFile.value) return

    savingPreviewFile.value = true
    try {
      const response = await workspaceApi.saveFile(selectedEntry.value.path, content)
      if (response.entry) selectedEntry.value = response.entry
      previewFile.value = {
        ...previewFile.value!,
        content
      }
      await loadWorkspaceEntries(currentPath.value)
      ElMessage.success('文件保存成功')
    } catch (error) {
      console.warn('保存工作区文件失败:', error)
      ElMessage.error((error as Error)?.message || '文件保存失败')
    } finally {
      savingPreviewFile.value = false
    }
  }

  const openCreateDirectoryModal = () => {
    if (!isPersonalSource.value) return
    newDirectoryName.value = ''
    createDirectoryModalVisible.value = true
  }

  const createDirectory = async () => {
    if (creatingDirectory.value) return
    const directoryName = newDirectoryName.value.trim()
    if (!directoryName) {
      ElMessage.warning('请输入文件夹名')
      return
    }

    creatingDirectory.value = true
    try {
      await workspaceApi.createDirectory(currentPath.value, directoryName)
      await loadWorkspaceEntries(currentPath.value)
      createDirectoryModalVisible.value = false
      newDirectoryName.value = ''
      ElMessage.success('文件夹创建成功')
    } catch (error) {
      console.warn('创建文件夹失败:', error)
      ElMessage.error((error as Error)?.message || '创建文件夹失败')
    } finally {
      creatingDirectory.value = false
    }
  }

  const openUploadFilePicker = () => {
    if (!isPersonalSource.value || uploadingFile.value) return
    if (uploadInputRef.value) {
      uploadInputRef.value.value = ''
      uploadInputRef.value.click()
    }
  }

  const handleUploadInputChange = async (event: Event) => {
    const input = event.target as HTMLInputElement
    const files = Array.from(input.files || [])
    if (!files.length || uploadingFile.value) return
    if (files.length > MAX_UPLOAD_FILES) {
      ElMessage.warning(`一次最多上传 ${MAX_UPLOAD_FILES} 个文件`)
      input.value = ''
      return
    }

    uploadingFile.value = true
    try {
      await workspaceApi.uploadFiles(currentPath.value, files)
      await loadWorkspaceEntries(currentPath.value)
      ElMessage.success(`${files.length} 个文件上传成功`)
    } catch (error) {
      console.warn('上传文件失败:', error)
      ElMessage.error((error as Error)?.message || '上传文件失败')
    } finally {
      uploadingFile.value = false
      input.value = ''
    }
  }

  const comparablePath = (path: string) => String(path || '/').replace(/\/$/, '') || '/'
  const isSameOrChildPath = (path: string, targetPath: string) => {
    const normalizedPath = comparablePath(path)
    const normalizedTargetPath = comparablePath(targetPath)
    return (
      normalizedPath === normalizedTargetPath ||
      normalizedPath.startsWith(`${normalizedTargetPath}/`)
    )
  }

  const confirmDeleteEntries = async (targetEntries: WorkspaceEntry[]) => {
    const validEntries = (targetEntries || []).filter(Boolean)
    if (!validEntries.length) return

    const isBatch = validEntries.length > 1
    const firstEntry = validEntries[0]
    const title = isBatch
      ? `确认删除选中的 ${validEntries.length} 项？`
      : firstEntry.is_dir
        ? `确认删除文件夹「${firstEntry.name}」？`
        : `确认删除文件「${firstEntry.name}」？`
    const content =
      isBatch || firstEntry.is_dir
        ? '将删除文件夹及其所有内容，删除后不可恢复。'
        : '删除后不可恢复。'

    try {
      await ElMessageBox.confirm(content, title, {
        confirmButtonText: '删除',
        cancelButtonText: '取消',
        type: 'warning'
      })
      await deleteEntries(validEntries)
    } catch {
      // 用户取消
    }
  }

  const deleteEntries = async (targetEntries: WorkspaceEntry[]) => {
    const paths = targetEntries.map((entry) => entry.path)
    deletingPaths.value = paths
    try {
      await Promise.all(paths.map((path) => workspaceApi.deletePath(path)))
      if (
        selectedEntry.value &&
        paths.some((path) => isSameOrChildPath(selectedEntry.value!.path, path))
      ) {
        closePreview()
      }
      clearWorkspaceSelection()
      await loadWorkspaceEntries(currentPath.value)
      ElMessage.success(paths.length > 1 ? '选中项删除成功' : '删除成功')
    } catch (error) {
      console.warn('删除工作区文件失败:', error)
      ElMessage.error((error as Error)?.message || '删除失败')
      await loadWorkspaceEntries(currentPath.value)
    } finally {
      deletingPaths.value = []
    }
  }

  const downloadEntry = async (entry: WorkspaceEntry) => {
    if (!entry || entry.is_dir) return

    try {
      const response =
        entry.source === 'knowledge'
          ? await workspaceApi.downloadKnowledgeFile(String(entry.kb_id), String(entry.file_id))
          : await workspaceApi.downloadFile(entry.path)
      const blob = await response.blob()
      const contentDisposition =
        response.headers.get('Content-Disposition') || response.headers.get('content-disposition')
      const filename = parseDownloadFilename(contentDisposition || '') || entry.name || 'download'
      const url = window.URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = filename
      document.body.appendChild(link)
      link.click()
      document.body.removeChild(link)
      window.URL.revokeObjectURL(url)
    } catch (error) {
      console.warn('下载文件失败:', error)
      ElMessage.error((error as Error)?.message || '下载文件失败')
    }
  }

  const downloadSelectedPreview = async () => {
    if (selectedEntry.value) await downloadEntry(selectedEntry.value)
  }

  onMounted(async () => {
    await Promise.all([loadWorkspaceEntries('/'), loadDatabases()])
  })

  onUnmounted(() => {
    revokePreviewObjectUrl()
  })
</script>

<style scoped>
  .workspace-page {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow: hidden;
    background: var(--el-bg-color);
  }

  .workspace-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    min-height: 56px;
    padding: 0 16px;
    border-bottom: 1px solid var(--el-border-color-lighter);
  }

  .header-title {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .header-title h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .upload-input {
    display: none;
  }

  .workspace-shell {
    position: relative;
    display: grid;
    grid-template-columns: 195px minmax(0, 1fr);
    flex: 1 1 auto;
    min-height: 0;
    overflow: hidden;
  }

  .workspace-shell.is-sidebar-collapsed {
    grid-template-columns: minmax(0, 1fr);
  }

  .workspace-sidebar-slot {
    position: relative;
    min-width: 0;
    min-height: 0;
  }

  .workspace-sidebar-slot :deep(.workspace-sidebar) {
    height: 100%;
  }

  .sidebar-collapse-action,
  .sidebar-expand-action {
    position: absolute;
    top: 50%;
    z-index: 4;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    border: 1px solid var(--el-border-color);
    background: var(--el-bg-color);
    color: var(--el-text-color-regular);
    cursor: pointer;
    transform: translateY(-50%);
    box-shadow: var(--el-box-shadow-light);
  }

  .sidebar-collapse-action {
    right: -13px;
    border-radius: 50%;
  }

  .sidebar-expand-action {
    left: 0;
    width: 22px;
    border-left: 0;
    border-radius: 0 12px 12px 0;
  }

  .workspace-main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .workspace-placeholder {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 360px;
    padding: 32px;
    color: var(--el-text-color-secondary);
    text-align: center;
  }

  .placeholder-icon {
    font-size: 32px;
  }

  .workspace-placeholder h2 {
    margin: 8px 0 0;
    color: var(--el-text-color-primary);
    font-size: 18px;
    font-weight: 600;
  }

  .workspace-placeholder p {
    max-width: 360px;
    margin: 0;
    font-size: 14px;
    line-height: 1.6;
  }
</style>

<style>
  .workspace-file-preview-modal.el-dialog {
    max-width: 1400px;
    padding: 0 !important;
    margin-top: 4vh !important;
    overflow: hidden;
    border-radius: 10px;
  }

  .workspace-file-preview-modal .el-dialog__header {
    display: none !important;
    height: 0;
    padding: 0 !important;
    margin: 0 !important;
  }

  .workspace-file-preview-modal .el-dialog__body {
    height: 88vh;
    max-height: 88vh;
    padding: 0 !important;
    margin: 0 !important;
    overflow: hidden;
  }

  .workspace-file-preview-modal .file-preview {
    height: 88vh;
  }

  .workspace-file-preview-modal .frame-preview {
    min-height: calc(88vh - 48px);
  }
</style>
