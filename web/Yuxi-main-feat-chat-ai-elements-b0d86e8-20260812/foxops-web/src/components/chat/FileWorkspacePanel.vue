<template>
  <div class="file-workspace" :class="{ 'is-fullscreen': isFullscreen, 'has-preview': !!selectedPath }">
    <!-- 工具栏 -->
    <div class="workspace-toolbar">
      <span class="workspace-title">文件</span>
      <div class="workspace-actions">
        <button class="ws-btn" title="刷新" @click="refresh"><ArtSvgIcon icon="ri:refresh-line" /></button>
        <button class="ws-btn" :title="isFullscreen ? '退出全屏' : '全屏'" @click="isFullscreen = !isFullscreen">
          <ArtSvgIcon :icon="isFullscreen ? 'ri:fullscreen-exit-line' : 'ri:fullscreen-line'" />
        </button>
        <button class="ws-btn" title="关闭" @click="$emit('close')"><ArtSvgIcon icon="ri:close-line" /></button>
      </div>
    </div>

    <!-- 上下布局：文件列表在上，预览区仅在选中文件后展开 -->
    <div class="workspace-body">
      <div class="file-list-section">
        <!-- 面包屑 / 返回上级 -->
        <div v-if="!isAtRoot || breadcrumb.length" class="file-breadcrumb">
          <button class="ws-btn" :disabled="isAtRoot" title="返回上级" @click="goUp">
            <ArtSvgIcon icon="ri:arrow-up-line" />
          </button>
          <span class="bc-text" :title="currentPath">user-data</span>
          <template v-for="(seg, idx) in breadcrumb" :key="idx">
            <ArtSvgIcon icon="ri:arrow-right-s-line" class="bc-sep" />
            <span class="bc-text" :title="seg">{{ seg }}</span>
          </template>
        </div>
        <div v-if="loading" class="tree-loading">加载中...</div>
        <div v-else-if="files.length === 0" class="tree-empty">暂无文件</div>
        <div v-else class="tree-list">
          <div
            v-for="entry in files"
            :key="entry.path"
            class="tree-item"
            :class="{ active: selectedPath === entry.path, 'is-dir': entry.is_dir }"
            @click="handleSelect(entry)"
          >
            <ArtSvgIcon :icon="entry.is_dir ? 'ri:folder-line' : 'ri:file-3-line'" class="tree-icon" />
            <span class="tree-name" :title="entry.name">{{ entry.name }}</span>
            <ArtSvgIcon v-if="entry.is_dir" icon="ri:arrow-right-s-line" class="tree-chevron" />
          </div>
        </div>
      </div>

      <!-- 预览区：仅在选中文件后出现 -->
      <div v-if="selectedPath" class="file-preview-section">
        <div class="preview-toolbar">
          <button class="ws-btn" title="返回文件列表" @click="closePreview">
            <ArtSvgIcon icon="ri:arrow-left-line" />
          </button>
          <span class="preview-name" :title="selectedName">{{ selectedName }}</span>
          <div class="preview-actions">
            <button class="ws-btn" title="下载" @click="downloadFile"><ArtSvgIcon icon="ri:download-line" /></button>
            <button class="ws-btn" title="保存到工作区" @click="saveToWorkspace"><ArtSvgIcon icon="ri:save-line" /></button>
          </div>
        </div>
        <div class="preview-content">
          <div v-if="previewLoading" class="preview-loading">加载中...</div>
          <div v-else-if="previewError" class="preview-error">{{ previewError }}</div>
          <pre v-else-if="previewContent" class="preview-text">{{ previewContent }}</pre>
          <div v-else class="preview-empty">无法预览此文件</div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, watch, onMounted, onUnmounted } from 'vue'
import { ElMessage } from 'element-plus'
import { threadApi } from '@/api/thread'
import { useUserStore } from '@/store/modules/user'

const props = defineProps({
  threadId: { type: String, default: '' },
  visible: { type: Boolean, default: false }
})

defineEmits(['close'])

const loading = ref(false)
const files = ref([])
const selectedPath = ref('')
const selectedName = ref('')
const previewContent = ref('')
const previewLoading = ref(false)
const previewError = ref('')
const isFullscreen = ref(false)
const ROOT_PATH = '/home/gem/user-data'
const currentPath = ref(ROOT_PATH)

const fetchFiles = async (path = currentPath.value) => {
  if (!props.threadId) return
  loading.value = true
  try {
    const resp = await threadApi.listThreadFiles(props.threadId, path, false)
    files.value = (resp?.files || []).map((f) => ({
      path: f.path || f.file_path || '',
      name: f.name || f.file_name || (f.path || '').split('/').pop() || '文件',
      is_dir: f.is_dir || f.type === 'dir',
      size: f.size || f.file_size
    }))
  } catch (e) {
    console.warn('Failed to fetch thread files:', e)
    files.value = []
  } finally {
    loading.value = false
  }
}

const refresh = () => {
  fetchFiles(currentPath.value)
}

// 进入子文件夹
const enterFolder = (entry) => {
  if (!entry.is_dir) return
  currentPath.value = entry.path
  // 进入新目录时清空预览
  closePreview()
  fetchFiles(entry.path)
}

// 返回上一级
const goUp = () => {
  if (currentPath.value === ROOT_PATH) return
  const parent = currentPath.value.replace(/\/+$/, '').split('/').slice(0, -1).join('/') || '/'
  currentPath.value = parent === '' ? ROOT_PATH : parent
  closePreview()
  fetchFiles(currentPath.value)
}

const isAtRoot = computed(() => currentPath.value === ROOT_PATH)
const breadcrumb = computed(() => {
  const rel = currentPath.value === ROOT_PATH
    ? ''
    : currentPath.value.slice(ROOT_PATH.length).replace(/^\/+/, '')
  return rel ? rel.split('/') : []
})

const handleSelect = async (entry) => {
  if (entry.is_dir) {
    enterFolder(entry)
    return
  }
  selectedPath.value = entry.path
  selectedName.value = entry.name
  previewLoading.value = true
  previewContent.value = ''
  previewError.value = ''
  try {
    const resp = await threadApi.readThreadFile(props.threadId, entry.path, 0, 2000)
    previewContent.value = resp?.content || ''
  } catch (e) {
    previewError.value = e?.message || '预览失败'
  } finally {
    previewLoading.value = false
  }
}

const closePreview = () => {
  selectedPath.value = ''
  selectedName.value = ''
  previewContent.value = ''
  previewError.value = ''
}

const downloadFile = async () => {
  if (!selectedPath.value || !props.threadId) return
  try {
    const blob = await threadApi.downloadThreadArtifact(props.threadId, selectedPath.value)
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = selectedName.value
    a.click()
    URL.revokeObjectURL(url)
  } catch (e) {
    ElMessage.error('下载失败: ' + (e?.message || ''))
  }
}

const saveToWorkspace = async () => {
  if (!selectedPath.value || !props.threadId) return
  try {
    await threadApi.saveThreadArtifactToWorkspace(props.threadId, selectedPath.value)
    ElMessage.success('已保存到工作区')
  } catch (e) {
    ElMessage.error('保存失败: ' + (e?.message || ''))
  }
}

watch(
  () => [props.visible, props.threadId],
  ([visible, tid]) => {
    if (visible && tid) {
      currentPath.value = ROOT_PATH
      closePreview()
      fetchFiles(ROOT_PATH)
    }
  }
)

onMounted(() => {
  if (props.visible && props.threadId) fetchFiles(ROOT_PATH)
})
</script>

<style scoped>
.file-workspace {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  background: var(--el-bg-color);
}
.file-workspace.is-fullscreen {
  position: fixed;
  top: 0; left: 0; right: 0; bottom: 0;
  z-index: 2000;
  height: 100vh;
  border-radius: 0;
}
.workspace-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 12px;
  background: var(--el-fill-color-light);
  border-bottom: 1px solid var(--el-border-color-lighter);
}
.workspace-title { font-size: 13px; font-weight: 600; }
.workspace-actions, .preview-actions { display: flex; gap: 2px; }
.ws-btn {
  border: none; background: transparent; cursor: pointer;
  padding: 4px; border-radius: 4px; color: var(--art-gray-500);
  display: flex; align-items: center; justify-content: center;
}
.ws-btn:hover { background: var(--el-fill-color); color: var(--art-gray-800); }

.workspace-body {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.file-list-section {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
}
.file-workspace.has-preview .file-list-section {
  flex: 0 0 45%;
  border-bottom: 1px solid var(--el-border-color-lighter);
}
.tree-loading, .tree-empty {
  padding: 16px; text-align: center; font-size: 13px; color: var(--art-gray-400);
}
.tree-item {
  display: flex; align-items: center; gap: 6px;
  padding: 5px 12px; cursor: pointer; font-size: 13px;
  transition: background 0.1s;
}
.tree-item:hover { background: var(--el-fill-color-light); }
.tree-item.active { background: var(--el-color-primary-light-9); color: var(--el-color-primary); }
.tree-item.is-dir { color: var(--art-gray-700); }
.tree-icon { font-size: 14px; flex-shrink: 0; }
.tree-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
.tree-chevron { font-size: 12px; color: var(--art-gray-400); flex-shrink: 0; }

/* 面包屑 / 返回上级 */
.file-breadcrumb {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 4px 6px;
  border-bottom: 1px solid var(--el-border-color-lighter);
  background: var(--el-fill-color-lighter);
  font-size: 12px;
  color: var(--art-gray-500);
  overflow: hidden;
}
.file-breadcrumb .ws-btn:disabled { opacity: 0.3; cursor: not-allowed; }
.file-breadcrumb .ws-btn:not(:disabled):hover { color: var(--el-color-primary); }
.bc-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 120px;
  flex-shrink: 0;
}
.bc-sep { font-size: 12px; color: var(--art-gray-300); flex-shrink: 0; }

.file-preview-section {
  flex: 1 1 55%;
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: var(--el-bg-color);
}
.file-workspace:not(.has-preview) .file-preview-section {
  display: none;
}
.preview-toolbar {
  display: flex; align-items: center; gap: 6px;
  padding: 4px 8px;
  border-bottom: 1px solid var(--el-border-color-lighter);
  background: var(--el-fill-color-lighter);
}
.preview-name {
  flex: 1;
  min-width: 0;
  font-size: 12px;
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.preview-actions { display: flex; gap: 2px; }
.preview-content { flex: 1; overflow: auto; padding: 12px; }
.preview-text {
  font-size: 13px; line-height: 1.5; white-space: pre-wrap; word-break: break-word;
  margin: 0; font-family: 'Monaco', 'Menlo', monospace;
}
.preview-loading, .preview-error, .preview-placeholder {
  padding: 24px; text-align: center; font-size: 13px; color: var(--art-gray-400);
}
.preview-error { color: var(--el-color-danger); }
</style>
