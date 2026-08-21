<template>
  <div v-if="normalizedArtifacts.length" class="artifacts-card">
    <div class="artifacts-header">
      <ArtSvgIcon icon="ri:gift-line" class="header-icon" />
      <span class="header-title">交付产物</span>
      <span class="header-count">{{ normalizedArtifacts.length }}</span>
    </div>
    <div class="artifacts-list">
      <div v-for="file in normalizedArtifacts" :key="file.path" class="artifact-item">
        <button class="artifact-main" :title="`预览 ${file.name}`" @click="openPreview(file)">
          <ArtSvgIcon icon="ri:file-3-line" class="artifact-icon" />
          <div class="artifact-meta">
            <div class="artifact-name">{{ file.name }}</div>
            <div class="artifact-desc">{{ getFileMetaLabel(file.path) }}</div>
          </div>
        </button>
        <div class="artifact-actions">
          <button class="artifact-action-btn" title="下载" @click.stop="downloadFile(file)">
            <ArtSvgIcon icon="ri:download-line" />
          </button>
          <button
            class="artifact-action-btn"
            :title="isSaving(file.path) ? '保存中' : '保存到工作区'"
            :disabled="isSaving(file.path)"
            @click.stop="saveToWorkspace(file)"
          >
            <ArtSvgIcon v-if="isSaving(file.path)" icon="ri:loader-4-line" class="spinning" />
            <ArtSvgIcon v-else icon="ri:save-line" />
          </button>
        </div>
      </div>
    </div>

    <!-- 预览弹窗 -->
    <ElDialog v-model="previewVisible" :title="previewFile?.name || '预览'" width="700px" @close="previewVisible = false">
      <div v-if="previewLoading" class="preview-loading">加载中...</div>
      <div v-else-if="previewError" class="preview-error">{{ previewError }}</div>
      <pre v-else-if="previewContent" class="preview-text">{{ previewContent }}</pre>
      <div v-else class="preview-empty">无法预览此文件</div>
    </ElDialog>
  </div>
</template>

<script setup>
import { ref, computed } from 'vue'
import { ElMessage } from 'element-plus'
import { threadApi } from '@/api/thread'

const props = defineProps({
  artifacts: { type: Array, default: () => [] },
  threadId: { type: String, default: '' }
})

const emit = defineEmits(['saved'])

const normalizedArtifacts = computed(() =>
  (props.artifacts || [])
    .filter((p) => typeof p === 'string' && p.trim())
    .map((p) => {
      const path = p.trim()
      return { path, name: path.split('/').pop() || path }
    })
)

const savingState = ref({})

const getFileMetaLabel = (path) => {
  const filename = String(path || '').split('/').pop() || ''
  if (!filename.includes('.')) return '交付文件'
  const ext = filename.split('.').pop()
  return ext ? `交付文件 · ${ext.toUpperCase()}` : '交付文件'
}

const isSaving = (path) => !!savingState.value[path]

// 预览
const previewVisible = ref(false)
const previewFile = ref(null)
const previewContent = ref('')
const previewLoading = ref(false)
const previewError = ref('')

const openPreview = async (file) => {
  previewFile.value = file
  previewVisible.value = true
  previewLoading.value = true
  previewContent.value = ''
  previewError.value = ''
  try {
    const resp = await threadApi.readThreadFile(props.threadId, file.path, 0, 2000)
    previewContent.value = resp?.content || ''
  } catch (e) {
    previewError.value = e?.message || '预览失败'
  } finally {
    previewLoading.value = false
  }
}

const downloadFile = async (file) => {
  if (!props.threadId || !file?.path) return
  try {
    const blob = await threadApi.downloadThreadArtifact(props.threadId, file.path)
    const url = window.URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = file.name
    a.click()
    window.URL.revokeObjectURL(url)
  } catch (e) {
    ElMessage.error('下载失败: ' + (e?.message || ''))
  }
}

const saveToWorkspace = async (file) => {
  if (!props.threadId || !file?.path) return
  savingState.value[file.path] = true
  try {
    await threadApi.saveThreadArtifactToWorkspace(props.threadId, file.path)
    ElMessage.success('已保存到工作区')
    emit('saved', file)
  } catch (e) {
    ElMessage.error('保存失败: ' + (e?.message || ''))
  } finally {
    savingState.value[file.path] = false
  }
}
</script>

<style scoped>
.artifacts-card {
  margin-top: 8px;
}
.artifacts-header {
  display: flex;
  align-items: center;
  gap: 4px;
  margin-bottom: 6px;
}
.header-icon { font-size: 14px; color: var(--art-gray-500); }
.header-title { font-size: 12px; font-weight: 600; color: var(--art-gray-600); }
.header-count { font-size: 11px; color: var(--art-gray-400); }

.artifacts-list { display: flex; flex-direction: column; gap: 4px; }

.artifact-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 10px;
  border: 1px solid var(--el-border-color-lighter);
  border-radius: 8px;
  background: var(--el-fill-color-lighter);
  transition: border-color 0.15s;
}
.artifact-item:hover { border-color: var(--el-color-primary-light-5); }

.artifact-main {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
  border: none;
  background: transparent;
  cursor: pointer;
  padding: 0;
  text-align: left;
}
.artifact-icon { font-size: 18px; color: var(--art-gray-500); flex-shrink: 0; }
.artifact-meta { flex: 1; min-width: 0; }
.artifact-name {
  font-size: 13px; font-weight: 500; color: var(--el-text-color-primary);
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.artifact-desc { font-size: 11px; color: var(--art-gray-400); }

.artifact-actions { display: flex; gap: 2px; flex-shrink: 0; }
.artifact-action-btn {
  border: none; background: transparent; cursor: pointer;
  padding: 4px; border-radius: 4px; color: var(--art-gray-500);
  display: flex; align-items: center; justify-content: center;
}
.artifact-action-btn:hover { background: var(--el-fill-color); color: var(--el-color-primary); }
.artifact-action-btn:disabled { opacity: 0.5; cursor: not-allowed; }

.spinning { animation: spin 1s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }

.preview-loading, .preview-error, .preview-empty {
  padding: 24px; text-align: center; font-size: 13px; color: var(--art-gray-400);
}
.preview-error { color: var(--el-color-danger); }
.preview-text {
  font-size: 13px; line-height: 1.5; white-space: pre-wrap; word-break: break-word;
  margin: 0; font-family: 'Monaco', 'Menlo', monospace;
  max-height: 500px; overflow: auto;
}
</style>
