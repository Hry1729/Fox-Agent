<template>
  <div class="knowledge-mindmap-panel">
    <div class="mindmap-stage" v-loading="loading && !hasMindmap">
      <!-- 有导图时的顶栏操作 -->
      <div v-if="hasMindmap && !unavailable" class="compact-actions">
        <div class="actions-right">
          <button
            type="button"
            class="action-btn"
            title="适应视图"
            :disabled="generating"
            @click="fitView"
          >
            <ArtSvgIcon icon="ri:fullscreen-line" />
          </button>
          <button
            type="button"
            class="action-btn"
            title="刷新"
            :disabled="generating"
            @click="loadMindmap"
          >
            <ArtSvgIcon icon="ri:refresh-line" :class="{ spin: loading }" />
          </button>
          <button
            type="button"
            class="action-btn"
            title="生成设置"
            :disabled="generating"
            @click="openGenerateDialog('regenerate')"
          >
            <ArtSvgIcon icon="ri:settings-3-line" />
          </button>
          <ElButton
            type="primary"
            size="small"
            :loading="generating"
            class="regen-btn"
            @click="openGenerateDialog('regenerate')"
          >
            <ArtSvgIcon icon="ri:sparkling-2-line" class="btn-icon" />
            重新生成
          </ElButton>
        </div>
      </div>

      <!-- 服务不可用 -->
      <div v-if="unavailable" class="mindmap-empty-state">
        <ArtSvgIcon icon="ri:error-warning-line" class="empty-icon is-warn" />
        <h3>知识导图不可用</h3>
        <p>请检查后端能力或 LITE 模式配置。</p>
        <ElButton :loading="loading" @click="loadMindmap">
          <ArtSvgIcon icon="ri:refresh-line" class="btn-icon" />
          重试
        </ElButton>
      </div>

      <!-- 生成中 -->
      <div v-else-if="generating && !hasMindmap" class="mindmap-empty-state">
        <ArtSvgIcon icon="ri:loader-4-line" class="empty-icon spin" />
        <h3>正在生成思维导图</h3>
        <p>AI 正在整理知识库结构，请稍候…</p>
      </div>

      <!-- 空白态：中央提示 + 提示词 + 生成按钮 -->
      <div v-else-if="!hasMindmap" class="mindmap-empty-state">
        <ArtSvgIcon icon="ri:mind-map" class="empty-icon" />
        <h3>暂无知识导图</h3>
        <p>从当前知识库内容生成结构化导图，可按需补充提示词。</p>
        <div class="empty-generate">
          <ElInput
            v-model="userPrompt"
            clearable
            placeholder="自定义提示词（可选）"
            class="empty-prompt"
            @keydown.enter.exact.prevent="generateFromEmpty"
          />
          <ElButton type="primary" :loading="generating" @click="generateFromEmpty">
            <ArtSvgIcon icon="ri:sparkling-2-line" class="btn-icon" />
            生成导图
          </ElButton>
          <button type="button" class="advanced-link" @click="openGenerateDialog('create')">
            <ArtSvgIcon icon="ri:settings-3-line" />
            高级选项
          </button>
        </div>
      </div>

      <!-- 有导图内容 -->
      <div v-else class="mindmap-main">
        <div ref="svgWrapRef" class="svg-wrap">
          <svg ref="svgRef" class="mindmap-svg"></svg>
        </div>
      </div>
    </div>

    <!-- 生成设置弹窗：文件选择 / 提示词 / 增量 -->
    <ElDialog
      v-model="showGenerateDialog"
      width="520px"
      destroy-on-close
      :show-close="false"
      class="extension-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
      footer-class="extension-dialog-footer"
    >
      <template #header>
        <ExtensionDialogHeader
          :title="generateDialogMode === 'create' ? '生成知识导图' : '重新生成知识导图'"
          @close="showGenerateDialog = false"
        />
      </template>
      <ElForm label-position="top" size="default">
        <ElFormItem label="选择文件">
          <ElSelect
            v-model="selectedFileIds"
            multiple
            filterable
            collapse-tags
            collapse-tags-tooltip
            clearable
            placeholder="留空则使用全部文件"
            class="full"
            :loading="filesLoading"
          >
            <ElOption
              v-for="file in files"
              :key="file.id || file.file_id"
              :label="file.filename || file.name || file.id || file.file_id"
              :value="file.id || file.file_id"
            />
          </ElSelect>
          <p class="form-hint">不选择时将基于知识库全部可用文件生成。</p>
        </ElFormItem>
        <ElFormItem label="自定义提示词">
          <ElInput
            v-model="dialogPrompt"
            type="textarea"
            :rows="3"
            placeholder="可选，用于引导导图结构或侧重点"
          />
        </ElFormItem>
        <ElFormItem v-if="hasMindmap" label="增量生成">
          <div class="incremental-row">
            <ElSwitch v-model="incremental" />
            <span class="form-hint inline">在现有导图基础上合并更新，而不是全量重建。</span>
          </div>
        </ElFormItem>
      </ElForm>
      <template #footer>
        <ElButton @click="showGenerateDialog = false">取消</ElButton>
        <ElButton type="primary" :loading="generating" @click="confirmGenerateDialog">
          <ArtSvgIcon icon="ri:sparkling-2-line" class="btn-icon" />
          {{ generateDialogMode === 'create' ? '开始生成' : '开始重新生成' }}
        </ElButton>
      </template>
    </ElDialog>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { knowledgeApi } from '@/api/knowledge'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import { useTaskerStore } from '@/store/modules/tasker'

  const props = defineProps<{ kbId: string }>()
  const tasker = useTaskerStore()

  const loading = ref(false)
  const generating = ref(false)
  const unavailable = ref(false)
  const filesLoading = ref(false)
  const files = ref<any[]>([])
  const selectedFileIds = ref<string[]>([])
  const userPrompt = ref('')
  const dialogPrompt = ref('')
  const incremental = ref(false)
  /** 后端返回的导图 JSON 树：{ content, children } */
  const mindmapData = ref<Record<string, any> | null>(null)
  const showGenerateDialog = ref(false)
  const generateDialogMode = ref<'create' | 'regenerate'>('create')
  const svgRef = ref<SVGSVGElement | null>(null)
  const svgWrapRef = ref<HTMLElement | null>(null)
  let markmapInstance: any = null

  const hasMindmap = computed(() => Boolean(mindmapData.value?.content))

  /** 将后端 JSON 树转为 Markmap 可用的 Markdown */
  const jsonToMarkdown = (node: any, level = 0): string => {
    if (!node || !node.content) return ''
    const indent = '#'.repeat(level + 1)
    let markdown = `${indent} ${node.content}\n\n`
    for (const child of node.children || []) {
      markdown += jsonToMarkdown(child, level + 1)
    }
    return markdown
  }

  /** 从接口响应中取出导图树（兼容直接对象 / 嵌套 data） */
  const extractMindmapTree = (payload: any): Record<string, any> | null => {
    if (!payload || typeof payload !== 'object') return null
    const candidate = payload.mindmap ?? payload.data?.mindmap ?? payload.data
    if (!candidate || typeof candidate !== 'object') return null
    if (typeof candidate === 'string') {
      try {
        const parsed = JSON.parse(candidate)
        return parsed?.content ? parsed : null
      } catch {
        return null
      }
    }
    return candidate.content ? candidate : null
  }

  const destroyMarkmap = () => {
    if (markmapInstance?.destroy) markmapInstance.destroy()
    markmapInstance = null
  }

  const fitView = () => {
    try {
      markmapInstance?.fit?.()
    } catch {
      // ignore
    }
  }

  const renderMarkmap = async (retryCount = 0) => {
    destroyMarkmap()
    if (!mindmapData.value) return
    if (!svgRef.value) {
      if (retryCount < 5) {
        setTimeout(() => renderMarkmap(retryCount + 1), 100)
      }
      return
    }
    try {
      const markdown = jsonToMarkdown(mindmapData.value)
      if (!markdown.trim()) return
      const [{ Markmap }, { Transformer }] = await Promise.all([
        import('markmap-view'),
        import('markmap-lib')
      ])
      const transformer = new Transformer()
      const { root } = transformer.transform(markdown)
      markmapInstance = Markmap.create(svgRef.value, undefined, root)
      markmapInstance.fit()
      setTimeout(() => markmapInstance?.fit?.(), 300)
    } catch (error) {
      ElMessage.warning((error as Error)?.message || 'Markmap 渲染失败，请确认已安装依赖')
    }
  }

  const loadFiles = async () => {
    filesLoading.value = true
    try {
      const result: any = await knowledgeApi.getMindmapFiles(props.kbId)
      files.value = result?.files || result?.items || result || []
      if (!Array.isArray(files.value)) files.value = []
    } catch {
      files.value = []
    } finally {
      filesLoading.value = false
    }
  }

  const loadMindmap = async () => {
    loading.value = true
    unavailable.value = false
    try {
      const result: any = await knowledgeApi.getMindmap(props.kbId)
      mindmapData.value = extractMindmapTree(result)
      await nextTick()
      if (mindmapData.value) {
        setTimeout(() => renderMarkmap(), 100)
      } else {
        destroyMarkmap()
      }
    } catch (error) {
      const msg = (error as Error)?.message || ''
      if (msg.includes('404') || msg.includes('不存在') || msg.includes('还没有生成')) {
        unavailable.value = false
        mindmapData.value = null
      } else {
        unavailable.value = true
        mindmapData.value = null
        ElMessage.warning(msg || '导图不可用')
      }
    } finally {
      loading.value = false
    }
  }

  const runGenerate = async (prompt: string, useIncremental: boolean) => {
    generating.value = true
    try {
      const result: any = await knowledgeApi.generateMindmap(
        props.kbId,
        selectedFileIds.value,
        prompt,
        useIncremental
      )
      const taskId = result?.task_id || result?.id
      if (taskId) {
        await tasker.trackTask(String(taskId))
        ElMessage.success('已提交导图生成任务')
      } else {
        const tree = extractMindmapTree(result)
        if (tree) mindmapData.value = tree
        await nextTick()
        setTimeout(() => renderMarkmap(), 100)
        ElMessage.success('导图已更新')
      }
      showGenerateDialog.value = false
    } catch (error) {
      ElMessage.error((error as Error)?.message || '生成失败')
    } finally {
      generating.value = false
    }
  }

  const generateFromEmpty = () => runGenerate(userPrompt.value, false)

  const openGenerateDialog = async (mode: 'create' | 'regenerate') => {
    generateDialogMode.value = mode
    dialogPrompt.value = userPrompt.value
    if (mode === 'create') incremental.value = false
    showGenerateDialog.value = true
    if (!files.value.length) await loadFiles()
  }

  const confirmGenerateDialog = async () => {
    userPrompt.value = dialogPrompt.value
    await runGenerate(dialogPrompt.value, hasMindmap.value ? incremental.value : false)
  }

  watch(
    () => props.kbId,
    async () => {
      selectedFileIds.value = []
      userPrompt.value = ''
      dialogPrompt.value = ''
      incremental.value = false
      await loadFiles()
      await loadMindmap()
    },
    { immediate: true }
  )

  watch(
    () =>
      Object.values(tasker.tasks)
        .map((t) => `${t.id}:${t.status}`)
        .join('|'),
    async () => {
      const done = Object.values(tasker.tasks).some((t) => tasker.isTerminal(t.status))
      if (done) await loadMindmap()
    }
  )

  onBeforeUnmount(destroyMarkmap)
</script>

<style scoped>
  .knowledge-mindmap-panel {
    height: 100%;
    min-height: 0;
  }

  .mindmap-stage {
    position: relative;
    height: 100%;
    min-height: 480px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
    overflow: hidden;
  }

  .compact-actions {
    position: absolute;
    top: 10px;
    right: 10px;
    z-index: 20;
    pointer-events: none;
  }

  .actions-right {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px;
    border-radius: 8px;
    border: 1px solid var(--el-border-color-lighter);
    background: color-mix(in srgb, var(--el-bg-color) 88%, transparent);
    backdrop-filter: blur(12px);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.06);
  }

  .action-btn {
    width: 32px;
    height: 32px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--el-text-color-regular);
    cursor: pointer;
  }

  .action-btn:hover:not(:disabled) {
    background: var(--el-fill-color);
    color: var(--el-color-primary);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .regen-btn {
    margin: 0 2px 0 4px;
  }

  .btn-icon {
    margin-right: 4px;
  }

  .mindmap-empty-state {
    height: 100%;
    min-height: 420px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 24px 16px;
    text-align: center;
    color: var(--el-text-color-secondary);
  }

  .empty-icon {
    font-size: 42px;
    color: var(--el-color-primary-light-5);
    margin-bottom: 4px;
  }

  .empty-icon.is-warn {
    color: var(--el-color-warning);
  }

  .mindmap-empty-state h3 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .mindmap-empty-state p {
    margin: 0 0 8px;
    font-size: 13px;
    max-width: 360px;
    line-height: 1.5;
  }

  .empty-generate {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    width: min(420px, 100%);
    margin-top: 4px;
  }

  .empty-prompt {
    width: 100%;
  }

  .advanced-link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-top: 2px;
    border: none;
    background: transparent;
    color: var(--el-text-color-secondary);
    font-size: 12px;
    cursor: pointer;
  }

  .advanced-link:hover {
    color: var(--el-color-primary);
  }

  .mindmap-main {
    height: 100%;
    min-height: 0;
  }

  .svg-wrap {
    width: 100%;
    height: 100%;
    min-height: 480px;
    overflow: hidden;
  }

  .mindmap-svg {
    width: 100%;
    height: 100%;
    display: block;
  }

  .full {
    width: 100%;
  }

  .form-hint {
    margin: 6px 0 0;
    font-size: 12px;
    color: var(--el-text-color-secondary);
    line-height: 1.4;
  }

  .form-hint.inline {
    margin: 0;
  }

  .incremental-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .spin {
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
