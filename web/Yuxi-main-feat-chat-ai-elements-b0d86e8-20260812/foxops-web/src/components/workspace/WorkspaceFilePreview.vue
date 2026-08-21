<template>
  <div class="file-preview" :class="{ fullscreen: isFullscreen }">
    <header class="preview-header">
      <div class="preview-title">
        <FileTypeIcon :name="file?.name || filePath" />
        <span :title="file?.name || filePath">{{ file?.name || filePath || '文件预览' }}</span>
      </div>
      <div class="preview-actions">
        <ElButton v-if="canEdit" size="small" :disabled="saving" @click="toggleEditing">
          {{ editing ? '取消编辑' : '编辑' }}
        </ElButton>
        <ElButton
          v-if="editing"
          size="small"
          type="primary"
          :loading="saving"
          @click="$emit('save', draft)"
        >
          保存
        </ElButton>
        <button type="button" class="preview-icon-btn" title="下载" @click="$emit('download')">
          <ArtSvgIcon icon="ri:download-line" />
        </button>
        <button
          type="button"
          class="preview-icon-btn"
          :title="isFullscreen ? '退出全屏' : '全屏'"
          @click="isFullscreen = !isFullscreen"
        >
          <ArtSvgIcon :icon="isFullscreen ? 'ri:fullscreen-exit-line' : 'ri:fullscreen-line'" />
        </button>
        <button
          type="button"
          class="preview-icon-btn preview-icon-btn--close"
          title="关闭"
          :disabled="saving"
          @click="$emit('close')"
        >
          <ArtSvgIcon icon="ri:close-line" />
        </button>
      </div>
    </header>

    <main class="preview-body">
      <ElInput
        v-if="editing"
        v-model="draft"
        type="textarea"
        resize="none"
        class="preview-editor"
      />
      <div
        v-else-if="file?.previewType === 'markdown'"
        class="markdown-body"
        v-html="renderMarkdown(file.content || '')"
      ></div>
      <pre v-else-if="file?.previewType === 'text'" class="text-preview">{{ file.content }}</pre>
      <img
        v-else-if="file?.previewType === 'image' && file.previewUrl"
        :src="file.previewUrl"
        :alt="file.name"
        class="image-preview"
      />
      <iframe
        v-else-if="file?.previewType === 'pdf' && file.previewUrl"
        :src="file.previewUrl"
        title="PDF 预览"
        class="frame-preview"
      />
      <iframe
        v-else-if="file?.previewType === 'html' && file.content"
        :srcdoc="file.content"
        sandbox="allow-same-origin"
        title="HTML 预览"
        class="frame-preview"
      />
      <iframe
        v-else-if="file?.previewType === 'html' && file.previewUrl"
        :src="file.previewUrl"
        sandbox="allow-same-origin"
        title="HTML 预览"
        class="frame-preview"
      />
      <SpreadsheetPreview
        v-else-if="file?.previewType === 'spreadsheet' && file.spreadsheet"
        :workbook="file.spreadsheet"
      />
      <ElEmpty
        v-else-if="file?.previewType === 'unsupported' || file?.supported === false"
        :description="file.message || '当前文件暂不支持预览，请下载后查看'"
      />
      <ElEmpty v-else description="选择文件以预览" />
    </main>
  </div>
</template>

<script setup lang="ts">
  import { renderMarkdown } from '@/utils/markdown'
  import FileTypeIcon from './FileTypeIcon.vue'
  import SpreadsheetPreview, { type SpreadsheetWorkbook } from './SpreadsheetPreview.vue'

  export interface PreviewFile {
    name: string
    path?: string
    content?: string | null
    previewType:
      | 'markdown'
      | 'text'
      | 'image'
      | 'pdf'
      | 'html'
      | 'spreadsheet'
      | 'office'
      | 'unsupported'
      | string
    previewUrl?: string
    message?: string
    supported?: boolean
    spreadsheet?: SpreadsheetWorkbook
  }

  const props = withDefaults(
    defineProps<{
      file?: PreviewFile | null
      filePath?: string
      editable?: boolean
      saving?: boolean
    }>(),
    { file: null, filePath: '', editable: false, saving: false }
  )

  defineEmits<{ close: []; save: [content: string]; download: [] }>()

  const editing = ref(false)
  const isFullscreen = ref(false)
  const draft = ref('')

  const canEdit = computed(
    () => props.editable && ['markdown', 'text'].includes(props.file?.previewType || '')
  )

  const toggleEditing = () => {
    if (props.saving) return
    editing.value = !editing.value
    if (!editing.value) {
      draft.value = props.file?.content || ''
    }
  }

  watch(
    () => props.file,
    (file) => {
      draft.value = file?.content || ''
      editing.value = false
      isFullscreen.value = false
    },
    { immediate: true }
  )
</script>

<style scoped>
  .file-preview {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    height: 100%;
    padding-top: 4px;
    background: var(--el-bg-color);
  }

  .file-preview.fullscreen {
    position: fixed;
    inset: 0;
    z-index: 3000;
  }

  .preview-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    flex: 0 0 48px;
    height: 48px;
    min-height: 48px;
    padding: 0 12px 0 14px;
    border-bottom: 1px solid var(--el-border-color-lighter);
    background: var(--el-fill-color-blank);
  }

  .preview-title,
  .preview-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .preview-title {
    gap: 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .preview-title span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .preview-actions {
    flex: 0 0 auto;
    gap: 4px;
  }

  .preview-icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    padding: 0;
    border: 1px solid var(--el-border-color);
    border-radius: 6px;
    background: var(--el-bg-color);
    color: var(--el-text-color-regular);
    font-size: 16px;
    cursor: pointer;
    transition:
      color 0.15s ease,
      background-color 0.15s ease,
      border-color 0.15s ease;
  }

  .preview-icon-btn:hover:not(:disabled) {
    color: var(--el-color-primary);
    border-color: var(--el-color-primary-light-5);
    background: var(--el-color-primary-light-9);
  }

  .preview-icon-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .preview-icon-btn--close:hover:not(:disabled) {
    color: var(--el-color-danger);
    border-color: var(--el-color-danger-light-5);
    background: var(--el-color-danger-light-9);
  }

  .preview-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 16px 18px;
  }

  .preview-editor {
    height: 100%;
  }

  .preview-editor :deep(textarea) {
    height: 100%;
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  }

  .text-preview {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font:
      13px/1.65 ui-monospace,
      SFMono-Regular,
      Consolas,
      monospace;
  }

  .markdown-body {
    line-height: 1.75;
  }

  .image-preview {
    display: block;
    max-width: 100%;
    max-height: 100%;
    margin: auto;
    object-fit: contain;
  }

  .frame-preview {
    width: 100%;
    height: 100%;
    min-height: 520px;
    border: 0;
  }
</style>
