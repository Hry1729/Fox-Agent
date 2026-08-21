<template>
  <ElDialog
    :model-value="modelValue"
    width="920px"
    destroy-on-close
    :show-close="false"
    class="extension-dialog"
    header-class="extension-dialog-header"
    body-class="extension-dialog-body"
    @close="$emit('update:modelValue', false)"
  >
    <template #header>
      <ExtensionDialogHeader title="文件详情" @close="$emit('update:modelValue', false)">
        <template #actions>
          <ElButton text size="small" :loading="downloading" @click="downloadOriginal">
            下载原文
          </ElButton>
          <ElButton text size="small" :disabled="!contentText" @click="downloadMarkdown">
            下载 Markdown
          </ElButton>
        </template>
      </ExtensionDialogHeader>
    </template>
    <div v-loading="loading">
      <ElTabs v-model="tab">
        <ElTabPane label="基本信息" name="basic">
          <ElDescriptions :column="1" border>
            <ElDescriptionsItem v-for="(value, key) in basicInfo" :key="key" :label="String(key)">
              {{ formatValue(value) }}
            </ElDescriptionsItem>
          </ElDescriptions>
        </ElTabPane>
        <ElTabPane label="原文件" name="source">
          <div v-loading="sourceLoading" class="source-preview">
            <img
              v-if="sourcePreview?.previewType === 'image' && sourcePreview.previewUrl"
              :src="sourcePreview.previewUrl"
              :alt="sourcePreview.name"
              class="source-image"
            />
            <iframe
              v-else-if="sourcePreview?.previewType === 'pdf' && sourcePreview.previewUrl"
              :src="sourcePreview.previewUrl"
              :title="sourcePreview.name"
              class="source-frame"
            />
            <SpreadsheetPreview
              v-else-if="sourcePreview?.previewType === 'spreadsheet' && sourcePreview.spreadsheet"
              :workbook="sourcePreview.spreadsheet"
            />
            <pre
              v-else-if="['text', 'markdown'].includes(sourcePreview?.previewType || '')"
              class="content-pre"
              >{{ sourcePreview?.content }}</pre
            >
            <ElEmpty
              v-else-if="!sourceLoading"
              :description="sourcePreview?.message || '当前文件暂不支持原文预览，请下载后查看'"
            />
          </div>
        </ElTabPane>
        <ElTabPane label="解析内容" name="content">
          <div
            v-if="contentText"
            class="markdown-content"
            v-html="renderMarkdown(contentText)"
          ></div>
          <ElEmpty v-else description="暂无解析内容" />
        </ElTabPane>
        <ElTabPane v-if="chunks.length" :label="`Chunks (${chunks.length})`" name="chunks">
          <div class="chunk-grid">
            <article v-for="(chunk, index) in chunks" :key="chunk.id || index" class="chunk-card">
              <strong>#{{ chunk.chunk_order_index ?? index }}</strong>
              <p>{{ chunk.content || '' }}</p>
            </article>
          </div>
        </ElTabPane>
      </ElTabs>
    </div>
  </ElDialog>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { knowledgeApi } from '@/api/knowledge'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import SpreadsheetPreview from '@/components/workspace/SpreadsheetPreview.vue'
  import type { PreviewFile } from '@/components/workspace/WorkspaceFilePreview.vue'
  import { renderMarkdown } from '@/utils/markdown'
  import { normalizePreviewResponse } from '@/utils/workspace'

  const props = defineProps<{ modelValue: boolean; kbId: string; fileId: string }>()
  defineEmits<{ 'update:modelValue': [value: boolean] }>()

  const loading = ref(false)
  const tab = ref('basic')
  const basicInfo = ref<Record<string, unknown>>({})
  const contentText = ref('')
  const contentInfo = ref<Record<string, any>>({})
  const sourcePreview = ref<PreviewFile | null>(null)
  const sourceLoading = ref(false)
  const sourceObjectUrl = ref('')
  const downloading = ref(false)

  const chunks = computed<any[]>(() => {
    const items = contentInfo.value?.chunks || contentInfo.value?.lines || []
    return Array.isArray(items) ? items : []
  })

  const formatValue = (value: unknown) => {
    if (value == null) return '-'
    if (typeof value === 'object') return JSON.stringify(value)
    return String(value)
  }

  const revokeSourceObjectUrl = () => {
    if (!sourceObjectUrl.value) return
    window.URL.revokeObjectURL(sourceObjectUrl.value)
    sourceObjectUrl.value = ''
  }

  const triggerDownload = (url: string, filename: string) => {
    const link = document.createElement('a')
    link.href = url
    link.download = filename
    document.body.appendChild(link)
    link.click()
    document.body.removeChild(link)
  }

  const downloadOriginal = async () => {
    if (!props.fileId || downloading.value) return
    downloading.value = true
    try {
      const response = await knowledgeApi.downloadDocument(props.kbId, props.fileId)
      const blob = await response.blob()
      const filename = String(
        basicInfo.value.filename ||
          basicInfo.value.name ||
          basicInfo.value.original_filename ||
          props.fileId
      )
      const url = window.URL.createObjectURL(blob)
      triggerDownload(url, filename)
      window.URL.revokeObjectURL(url)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '下载原文失败')
    } finally {
      downloading.value = false
    }
  }

  const downloadMarkdown = () => {
    if (!contentText.value) return
    const originalName = String(
      basicInfo.value.filename ||
        basicInfo.value.name ||
        basicInfo.value.original_filename ||
        props.fileId
    )
    const filename = `${originalName.replace(/\.[^.]+$/, '') || 'document'}.md`
    const url = window.URL.createObjectURL(
      new Blob([contentText.value], { type: 'text/markdown;charset=utf-8' })
    )
    triggerDownload(url, filename)
    window.URL.revokeObjectURL(url)
  }

  const loadSourcePreview = async () => {
    if (!props.fileId || sourceLoading.value) return
    sourceLoading.value = true
    try {
      const response = await knowledgeApi.downloadDocument(props.kbId, props.fileId)
      const name = String(
        basicInfo.value.filename ||
          basicInfo.value.name ||
          basicInfo.value.original_filename ||
          props.fileId
      )
      const preview = (await normalizePreviewResponse(response, { name })) as PreviewFile
      revokeSourceObjectUrl()
      if (preview.previewUrl) sourceObjectUrl.value = preview.previewUrl
      sourcePreview.value = preview
    } catch (error) {
      sourcePreview.value = {
        name: String(basicInfo.value.filename || props.fileId),
        previewType: 'unsupported',
        supported: false,
        message: (error as Error)?.message || '原文件预览加载失败'
      }
    } finally {
      sourceLoading.value = false
    }
  }

  const load = async () => {
    if (!props.fileId) return
    loading.value = true
    sourcePreview.value = null
    contentInfo.value = {}
    contentText.value = ''
    revokeSourceObjectUrl()
    try {
      const [basic, content]: any[] = await Promise.all([
        knowledgeApi.getDocumentBasicInfo(props.kbId, props.fileId),
        knowledgeApi.getDocumentContent(props.kbId, props.fileId).catch(() => null)
      ])
      basicInfo.value = basic || {}
      contentInfo.value = content || {}
      contentText.value =
        content?.content ||
        content?.text ||
        (Array.isArray(content?.lines)
          ? content.lines.map((item: any) => item?.content || '').join('\n\n')
          : '')
      await loadSourcePreview()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载详情失败')
    } finally {
      loading.value = false
    }
  }

  watch(
    () => [props.modelValue, props.fileId],
    ([open]) => {
      if (open) {
        tab.value = 'basic'
        load()
      } else {
        revokeSourceObjectUrl()
      }
    }
  )

  onBeforeUnmount(revokeSourceObjectUrl)
</script>

<style scoped>
  .source-preview {
    min-height: 440px;
    height: 58vh;
    overflow: hidden;
  }

  .source-image {
    display: block;
    max-width: 100%;
    max-height: 100%;
    margin: auto;
    object-fit: contain;
  }

  .source-frame {
    width: 100%;
    height: 100%;
    border: 0;
  }

  .markdown-content {
    max-height: 58vh;
    overflow: auto;
    line-height: 1.75;
  }

  .content-pre {
    max-height: 50vh;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    margin: 0;
    font-size: 12px;
  }

  .chunk-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 12px;
    max-height: 58vh;
    overflow: auto;
  }

  .chunk-card {
    padding: 12px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 8px;
    background: var(--el-fill-color-lighter);
  }

  .chunk-card strong {
    color: var(--el-color-primary);
    font-size: 12px;
  }

  .chunk-card p {
    margin: 8px 0 0;
    color: var(--el-text-color-regular);
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
