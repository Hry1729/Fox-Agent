<template>
  <aside class="workspace-preview-pane">
    <WorkspaceFilePreview
      v-if="file"
      :file="file"
      :file-path="filePath"
      :editable="editable"
      :saving="saving"
      @close="$emit('close')"
      @save="$emit('save', $event)"
      @download="$emit('download')"
    />
    <div v-else-if="loading" class="preview-state">
      <ElIcon class="is-loading"><Loading /></ElIcon>
      <span>正在加载预览...</span>
    </div>
    <div v-else class="preview-empty">
      <ArtSvgIcon icon="ri:file-search-line" class="empty-icon" />
      <h3>选择文件以预览</h3>
      <p>支持 Markdown、TXT 编辑，其他格式保持只读预览。</p>
    </div>
  </aside>
</template>

<script setup lang="ts">
  import { Loading } from '@element-plus/icons-vue'
  import WorkspaceFilePreview, { type PreviewFile } from './WorkspaceFilePreview.vue'

  withDefaults(
    defineProps<{
      file?: PreviewFile | null
      filePath?: string
      loading?: boolean
      editable?: boolean
      saving?: boolean
    }>(),
    { file: null, filePath: '', loading: false, editable: false, saving: false }
  )

  defineEmits<{ close: []; save: [content: string]; download: [] }>()
</script>

<style scoped>
  .workspace-preview-pane {
    min-width: 0;
    min-height: 0;
    height: 100%;
    overflow: hidden;
    border-left: 1px solid var(--el-border-color-lighter);
    background: var(--el-bg-color);
  }

  .preview-state,
  .preview-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    height: 100%;
    min-height: 260px;
    padding: 24px;
    color: var(--el-text-color-secondary);
    text-align: center;
  }

  .empty-icon {
    font-size: 28px;
  }

  .preview-empty h3 {
    margin: 6px 0 0;
    color: var(--el-text-color-primary);
    font-size: 15px;
    font-weight: 600;
  }

  .preview-empty p {
    max-width: 240px;
    margin: 0;
    font-size: 13px;
    line-height: 1.6;
  }
</style>
