<template>
  <div class="knowledge-benchmark-panel" v-loading="loading && !benchmarks.length">
    <div class="panel-header">
      <div class="header-left">
        <ElButton type="primary" @click="uploadVisible = true">
          <ArtSvgIcon icon="ri:upload-2-line" class="btn-icon" />
          上传基准
        </ElButton>
        <ElButton @click="generateVisible = true">
          <ArtSvgIcon icon="ri:robot-2-line" class="btn-icon" />
          自动生成
        </ElButton>
        <span class="total-count">{{ benchmarks.length }} 个基准</span>
      </div>
      <ElButton text @click="loadBenchmarks()">
        <ArtSvgIcon icon="ri:refresh-line" :class="{ spin: loading }" class="btn-icon" />
        刷新
      </ElButton>
    </div>

    <div v-if="!loading && !benchmarks.length" class="empty-state panel-body">
      <ArtSvgIcon icon="ri:clipboard-line" class="empty-icon" />
      <h3>暂无评估基准</h3>
      <p>上传数据集，或从当前知识库自动生成评估问题。</p>
      <div class="empty-actions">
        <ElButton type="primary" @click="uploadVisible = true">
          <ArtSvgIcon icon="ri:upload-2-line" class="btn-icon" />
          上传基准
        </ElButton>
        <ElButton @click="generateVisible = true">
          <ArtSvgIcon icon="ri:robot-2-line" class="btn-icon" />
          自动生成
        </ElButton>
      </div>
    </div>

    <div v-else-if="benchmarks.length" class="panel-body">
      <div class="benchmark-grid">
        <div
          v-for="item in benchmarks"
          :key="item.dataset_id || item.id"
          class="benchmark-card"
          :class="{ disabled: !isCompleted(item) }"
          @click="isCompleted(item) && openPreview(item)"
        >
          <div class="card-top">
            <h4>{{ item.name || '未命名基准' }}</h4>
            <ElDropdown trigger="click" @command="(cmd) => onCardCommand(cmd, item)">
              <button type="button" class="more-btn" @click.stop>
                <ArtSvgIcon icon="ri:more-2-fill" />
              </button>
              <template #dropdown>
                <ElDropdownMenu>
                  <ElDropdownItem command="download" :disabled="!isCompleted(item)">
                    下载
                  </ElDropdownItem>
                  <ElDropdownItem command="delete" divided :disabled="isBuilding(item)">
                    删除
                  </ElDropdownItem>
                </ElDropdownMenu>
              </template>
            </ElDropdown>
          </div>
          <p class="card-desc">{{ item.description || '暂无描述' }}</p>
          <div class="card-tags">
            <ElTag v-if="item.has_gold_chunks && !item.has_gold_answers" size="small" type="primary">
              检索评估
            </ElTag>
            <ElTag v-if="item.has_gold_answers && !item.has_gold_chunks" size="small" type="warning">
              问答评估
            </ElTag>
            <ElTag v-if="!item.has_gold_chunks && !item.has_gold_answers" size="small">仅查询</ElTag>
            <ElTag v-if="item.has_gold_chunks" size="small" type="success">Gold Chunks</ElTag>
            <ElTag v-if="item.has_gold_answers" size="small" type="success">Gold Answer</ElTag>
            <ElTag size="small" type="info">{{ sourceText(item) }}</ElTag>
            <ElTag v-if="!isCompleted(item)" size="small" :type="statusTagType(item)">
              {{ statusText(item) }}
            </ElTag>
          </div>
          <div class="card-footer">
            <span>{{ formatDate(item.created_at) }}</span>
            <div v-if="isBuilding(item)" class="build-progress">
              <ElProgress :percentage="buildProgress(item)" :show-text="false" />
              <span>{{ buildMessage(item) }}</span>
            </div>
            <span v-else>{{ item.item_count ?? 0 }} 个问题</span>
          </div>
        </div>
      </div>
    </div>

    <div v-else class="panel-body panel-loading" />
    <ElDialog
      v-model="uploadVisible"
      width="520px"
      destroy-on-close
      :show-close="false"
      class="extension-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
      footer-class="extension-dialog-footer"
    >
      <template #header>
        <ExtensionDialogHeader title="上传评估基准" @close="uploadVisible = false" />
      </template>
      <ElForm label-position="top" class="compact-form" size="default">
        <ElFormItem label="基准名称" required>
          <ElInput v-model="uploadForm.name" placeholder="请输入评估基准名称" maxlength="100" />
        </ElFormItem>
        <ElFormItem label="描述">
          <ElInput
            v-model="uploadForm.description"
            type="textarea"
            :rows="2"
            placeholder="可选"
          />
        </ElFormItem>
        <ElFormItem label="基准文件" required class="upload-file-item">
          <ElUpload
            drag
            class="benchmark-uploader"
            :auto-upload="false"
            :limit="1"
            accept=".jsonl"
            :on-change="onUploadFileChange"
            :on-remove="() => (uploadForm.file = null)"
          >
            <ArtSvgIcon icon="ri:upload-cloud-2-line" class="upload-icon" />
            <div class="el-upload__text">点击或拖拽 JSONL 文件到此处</div>
            <template #tip>
              <div class="el-upload__tip">每行一个 JSON 对象，仅支持 .jsonl</div>
            </template>
          </ElUpload>
        </ElFormItem>
      </ElForm>
      <template #footer>
        <ElButton @click="uploadVisible = false">取消</ElButton>
        <ElButton type="primary" :loading="uploading" @click="submitUpload">上传</ElButton>
      </template>
    </ElDialog>

    <!-- 自动生成弹窗 -->
    <ElDialog
      v-model="generateVisible"
      width="560px"
      destroy-on-close
      :show-close="false"
      class="extension-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
      footer-class="extension-dialog-footer"
    >
      <template #header>
        <ExtensionDialogHeader title="自动生成评估基准" @close="generateVisible = false" />
      </template>
      <ElForm label-position="top" class="compact-form" size="default">
        <ElFormItem label="基准名称" required>
          <ElInput v-model="generateForm.name" maxlength="100" />
        </ElFormItem>
        <ElFormItem label="描述">
          <ElInput v-model="generateForm.description" type="textarea" :rows="2" />
        </ElFormItem>
        <ElFormItem label="构建方式" class="mode-cards-item">
          <div class="mode-cards">
            <button
              v-for="option in generationModeOptions"
              :key="option.value"
              type="button"
              class="mode-card"
              :class="{
                active: generateForm.generation_mode === option.value,
                disabled: option.disabled
              }"
              :disabled="option.disabled"
              @click="selectGenerationMode(option)"
            >
              <div class="mode-card-header">
                <ArtSvgIcon :icon="option.icon" class="mode-icon" />
                <strong class="mode-title">{{ option.label }}</strong>
                <ElTag v-if="option.tag" size="small" class="mode-tag">{{ option.tag }}</ElTag>
              </div>
              <p class="mode-description">{{ option.description }}</p>
              <p v-if="option.helper" class="mode-helper" :class="{ warning: option.disabled }">
                {{ option.helper }}
              </p>
            </button>
          </div>
        </ElFormItem>
        <ElFormItem label="LLM 模型" required>
          <ElSelect
            v-model="generateForm.llm_model_spec"
            filterable
            clearable
            placeholder="选择用于生成问题的模型"
            class="full"
            :loading="chatModelsLoading"
          >
            <ElOptionGroup
              v-for="(group, providerId) in chatModels"
              :key="providerId"
              :label="group.provider_display_name || String(providerId)"
            >
              <ElOption
                v-for="model in group.models"
                :key="model.spec"
                :label="model.display_name || model.spec"
                :value="model.spec"
              />
            </ElOptionGroup>
          </ElSelect>
        </ElFormItem>
        <div class="param-grid">
          <ElFormItem label="问题数量">
            <ElInputNumber v-model="generateForm.count" :min="1" :max="100" controls-position="right" class="full" />
          </ElFormItem>
          <ElFormItem label="候选 Chunk 数">
            <ElInputNumber
              v-model="generateForm.neighbors_count"
              :min="0"
              :max="10"
              controls-position="right"
              class="full"
            />
          </ElFormItem>
          <ElFormItem label="构建并发数">
            <ElInputNumber
              v-model="generateForm.concurrency_count"
              :min="1"
              :max="20"
              controls-position="right"
              class="full"
            />
          </ElFormItem>
        </div>
        <ElFormItem
          v-if="generateForm.generation_mode === 'graph_enhanced'"
          label="每轮扩展 Chunk"
          class="graph-expand-item"
        >
          <ElInputNumber
            v-model="generateForm.graph_expand_top_k"
            :min="1"
            :max="3"
            controls-position="right"
            class="full"
          />
        </ElFormItem>
      </ElForm>
      <template #footer>
        <ElButton @click="generateVisible = false">取消</ElButton>
        <ElButton type="primary" :loading="generating" @click="submitGenerate">确定</ElButton>
      </template>
    </ElDialog>

    <!-- 预览弹窗 -->
    <ElDialog
      v-model="previewVisible"
      width="1080px"
      top="4vh"
      align-center
      destroy-on-close
      :show-close="false"
      class="extension-dialog benchmark-preview-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
    >
      <template #header>
        <ExtensionDialogHeader title="评估基准详情" @close="previewVisible = false" />
      </template>
      <div v-if="previewMeta" class="preview-panel">
        <div class="preview-header">
          <div class="preview-header-text">
            <h3 class="preview-title">{{ previewMeta.name }}</h3>
            <p class="preview-desc">{{ previewMeta.description || '暂无描述' }}</p>
          </div>
          <div class="preview-meta">
            <span class="meta-item">
              <span class="meta-label">问题数</span>
              <strong>{{ previewMeta.item_count ?? previewTotal }}</strong>
            </span>
            <span class="meta-item">
              <span class="meta-label">Gold Chunks</span>
              <ElTag size="small" :type="previewMeta.has_gold_chunks ? 'success' : 'info'">
                {{ previewMeta.has_gold_chunks ? '有' : '无' }}
              </ElTag>
            </span>
            <span class="meta-item">
              <span class="meta-label">Gold Answer</span>
              <ElTag size="small" :type="previewMeta.has_gold_answers ? 'success' : 'info'">
                {{ previewMeta.has_gold_answers ? '有' : '无' }}
              </ElTag>
            </span>
          </div>
        </div>

        <div class="preview-table-section">
          <div class="preview-table-title">
            <h4>问题列表</h4>
            <span>共 {{ previewTotal }} 条</span>
          </div>
          <ElCard class="art-table-card benchmark-preview-table-card" shadow="never">
            <ArtTable
              :loading="previewLoading"
              :data="previewQuestions"
              :columns="previewColumns"
              :pagination="previewPagination"
              :height="previewTableHeight"
              :show-table-header="false"
              empty-text="暂无问题"
              empty-height="280px"
              size="small"
              @pagination:size-change="handlePreviewSizeChange"
              @pagination:current-change="handlePreviewCurrentChange"
            >
              <template #question="{ row }">
                <BenchmarkPreviewCell :text="getQuestionText(row)" />
              </template>
              <template #gold_chunks="{ row }">
                <BenchmarkPreviewCell :text="getGoldChunksText(row)" placeholder="-" />
              </template>
              <template #gold_answer="{ row }">
                <BenchmarkPreviewCell :text="getGoldAnswerText(row)" placeholder="-" />
              </template>
            </ArtTable>
          </ElCard>
        </div>
      </div>
    </ElDialog>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { evaluationApi } from '@/api/knowledge'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import BenchmarkPreviewCell from '@/components/extensions/knowledge/BenchmarkPreviewCell.vue'
  import type { ColumnOption } from '@/types'
  import { graphApi } from '@/api/graph'
  import { modelProviderApi } from '@/api/model'
  import { useTaskerStore } from '@/store/modules/tasker'
  import { unwrapApiData, unwrapList } from '@/utils/apiData'
  import { parseDownloadFilename } from '@/utils/workspace'

  const props = defineProps<{ kbId: string }>()
  defineEmits<{ 'goto-evaluation': [] }>()
  const tasker = useTaskerStore()

  const loading = ref(false)
  const uploading = ref(false)
  const generating = ref(false)
  const chatModelsLoading = ref(false)
  const chatModels = ref<
    Record<string, { provider_display_name?: string; models: Array<{ spec: string; display_name?: string }> }>
  >({})
  const benchmarks = ref<any[]>([])
  const uploadVisible = ref(false)
  const generateVisible = ref(false)
  const previewVisible = ref(false)
  const previewLoading = ref(false)
  const previewMeta = ref<any>(null)
  const previewQuestions = ref<any[]>([])
  const previewPage = ref(1)
  const previewPageSize = ref(20)
  const previewTotal = ref(0)
  const previewTableHeight = 520
  const graphIndexedChunks = ref(0)
  let pollTimer: ReturnType<typeof setInterval> | null = null

  const defaultName = () => {
    const today = new Date().toISOString().slice(0, 10)
    const suffix = Math.random().toString(36).slice(2, 6)
    return `Test-${today}-${suffix}`
  }

  const uploadForm = reactive<{ name: string; description: string; file: File | null }>({
    name: '',
    description: '',
    file: null
  })

  const generateForm = reactive({
    name: defaultName(),
    description: '',
    count: 10,
    neighbors_count: 1,
    concurrency_count: 10,
    generation_mode: 'vector',
    graph_expand_top_k: 1,
    llm_model_spec: ''
  })

  const graphEnhancedDisabled = computed(() => graphIndexedChunks.value <= 0)

  const generationModeOptions = computed(() => [
    {
      value: 'vector',
      label: '向量构建',
      tag: '默认',
      icon: 'ri:database-2-line',
      description: '基于向量相似度召回 Chunk，稳定适用于所有知识库。',
      helper: '适合快速生成通用评估基准。',
      disabled: false
    },
    {
      value: 'graph_enhanced',
      label: '图增强构建',
      tag: '图谱',
      icon: 'ri:share-line',
      description: '在向量召回基础上结合知识图谱扩展相关 Chunk。',
      helper: graphEnhancedDisabled.value
        ? '当前知识库尚未完成图谱构建，暂不能使用图增强构建'
        : `已构建图谱的 Chunk：${graphIndexedChunks.value}`,
      disabled: graphEnhancedDisabled.value
    }
  ])

  const selectGenerationMode = (option: { value: string; disabled?: boolean }) => {
    if (option.disabled) return
    generateForm.generation_mode = option.value
  }

  const buildMeta = (item: any) => item?.build_metadata || {}
  const isCompleted = (item: any) => (buildMeta(item).status || 'completed') === 'completed'
  const isBuilding = (item: any) => {
    const s = buildMeta(item).status
    return s === 'pending' || s === 'running' || s === 'building'
  }
  const buildProgress = (item: any) => {
    const p = Number(buildMeta(item).progress)
    if (!Number.isFinite(p)) return 0
    return Math.max(0, Math.min(100, Math.round(p <= 1 ? p * 100 : p)))
  }
  const buildMessage = (item: any) =>
    buildMeta(item).error_message || buildMeta(item).message || statusText(item)
  const sourceText = (item: any) => {
    const source = buildMeta(item).source
    if (source === 'upload') return '上传'
    if (source === 'generate' || source === 'auto') return '自动生成'
    return source || '未知来源'
  }
  const statusText = (item: any) => {
    const map: Record<string, string> = {
      pending: '排队中',
      running: '构建中',
      building: '构建中',
      failed: '失败',
      completed: '已完成'
    }
    return map[buildMeta(item).status] || buildMeta(item).status || '未知'
  }
  const statusTagType = (item: any) => {
    const s = buildMeta(item).status
    if (s === 'failed') return 'danger'
    if (s === 'completed') return 'success'
    return 'warning'
  }
  const formatDate = (value?: string) => {
    if (!value) return '-'
    const d = new Date(value)
    return Number.isNaN(d.getTime()) ? String(value) : d.toLocaleString()
  }

  const stopPoll = () => {
    if (pollTimer) {
      clearInterval(pollTimer)
      pollTimer = null
    }
  }

  const syncPoll = () => {
    stopPoll()
    if (benchmarks.value.some(isBuilding)) {
      pollTimer = setInterval(() => loadBenchmarks(true), 3000)
    }
  }

  const loadBenchmarks = async (silent = false) => {
    if (!silent) loading.value = true
    try {
      const result: any = await evaluationApi.listDatasets(props.kbId)
      benchmarks.value = unwrapList(result).map((item: any) => ({
        ...item,
        dataset_id: item.dataset_id || item.id
      }))
      syncPoll()
    } catch (error) {
      if (!silent) ElMessage.error((error as Error)?.message || '加载评估基准失败')
    } finally {
      loading.value = false
    }
  }

  const loadGraphStatus = async () => {
    try {
      const result: any = await graphApi.getGraphBuildStatus(props.kbId)
      const data = result?.data || result
      graphIndexedChunks.value = Number(data?.indexed_chunks || 0)
    } catch {
      graphIndexedChunks.value = 0
    }
    if (graphEnhancedDisabled.value && generateForm.generation_mode === 'graph_enhanced') {
      generateForm.generation_mode = 'vector'
    }
  }

  const loadChatModels = async () => {
    chatModelsLoading.value = true
    try {
      const result: any = await modelProviderApi.getV2Models('chat')
      const data = result?.data || unwrapApiData(result, result) || {}
      const mapped: Record<
        string,
        { provider_display_name?: string; models: Array<{ spec: string; display_name?: string }> }
      > = {}
      Object.entries(data).forEach(([providerId, value]: [string, any]) => {
        if (Array.isArray(value)) {
          mapped[providerId] = { provider_display_name: providerId, models: value }
        } else {
          mapped[providerId] = {
            provider_display_name: value?.provider_display_name || providerId,
            models: value?.models || []
          }
        }
      })
      chatModels.value = mapped
      if (!generateForm.llm_model_spec) {
        for (const group of Object.values(mapped)) {
          if (group.models?.[0]?.spec) {
            generateForm.llm_model_spec = group.models[0].spec
            break
          }
        }
      }
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载模型列表失败')
      chatModels.value = {}
    } finally {
      chatModelsLoading.value = false
    }
  }

  const onUploadFileChange = (file: any) => {
    uploadForm.file = file?.raw || null
    if (!uploadForm.name && file?.name) {
      uploadForm.name = String(file.name).replace(/\.jsonl$/i, '')
    }
  }

  const submitUpload = async () => {
    if (!uploadForm.name.trim()) return ElMessage.warning('请输入基准名称')
    if (!uploadForm.file) return ElMessage.warning('请选择 JSONL 文件')
    uploading.value = true
    try {
      await evaluationApi.uploadDataset(props.kbId, uploadForm.file, {
        name: uploadForm.name.trim(),
        description: uploadForm.description
      })
      ElMessage.success('上传成功')
      uploadVisible.value = false
      uploadForm.name = ''
      uploadForm.description = ''
      uploadForm.file = null
      await loadBenchmarks()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '上传失败')
    } finally {
      uploading.value = false
    }
  }

  const submitGenerate = async () => {
    if (!generateForm.name.trim()) return ElMessage.warning('请输入基准名称')
    if (!generateForm.llm_model_spec) return ElMessage.warning('请选择 LLM 模型')
    generating.value = true
    try {
      const result: any = await evaluationApi.generateDataset(props.kbId, { ...generateForm })
      const taskId = result?.task_id || result?.id
      if (taskId) await tasker.trackTask(String(taskId))
      ElMessage.success('生成任务已提交')
      generateVisible.value = false
      generateForm.name = defaultName()
      await loadBenchmarks()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '生成失败')
    } finally {
      generating.value = false
    }
  }

  const openPreview = async (item: any) => {
    previewMeta.value = item
    previewPage.value = 1
    previewVisible.value = true
    await loadPreviewQuestions()
  }

  const previewPagination = computed(() => ({
    current: previewPage.value,
    size: previewPageSize.value,
    total: previewTotal.value
  }))

  const previewColumns = computed<ColumnOption[]>(() => [
    { type: 'globalIndex', label: '序号', width: 68, align: 'center' },
    { prop: 'question', label: '问题', minWidth: 280, useSlot: true },
    { prop: 'gold_chunks', label: 'Gold Chunks', minWidth: 200, useSlot: true },
    { prop: 'gold_answer', label: 'Gold Answer', minWidth: 320, useSlot: true }
  ])

  const getQuestionText = (row: any) => String(row?.question || row?.query || '').trim()

  const getGoldChunksText = (row: any) => {
    const ids = row?.gold_chunk_ids || row?.gold_chunks
    if (!Array.isArray(ids) || !ids.length) return ''
    return ids.map((item: unknown) => String(item)).join(', ')
  }

  const getGoldAnswerText = (row: any) => String(row?.gold_answer || '').trim()

  const handlePreviewSizeChange = (size: number) => {
    previewPageSize.value = size
    previewPage.value = 1
    loadPreviewQuestions()
  }

  const handlePreviewCurrentChange = (page: number) => {
    previewPage.value = page
    loadPreviewQuestions()
  }

  const loadPreviewQuestions = async () => {
    if (!previewMeta.value) return
    previewLoading.value = true
    try {
      const id = previewMeta.value.dataset_id || previewMeta.value.id
      const result: any = await evaluationApi.getDataset(props.kbId, id, previewPage.value, previewPageSize.value)
      const data = result?.data || result
      previewQuestions.value = unwrapList(data?.items ? data : result)
      previewTotal.value = Number(
        data?.pagination?.total_items ?? data?.total ?? previewMeta.value?.item_count ?? previewQuestions.value.length
      )
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载基准详情失败')
    } finally {
      previewLoading.value = false
    }
  }

  const downloadDataset = async (item: any) => {
    try {
      const id = item.dataset_id || item.id
      const response = await evaluationApi.downloadDataset(id)
      const blob = await response.blob()
      const filename =
        parseDownloadFilename(
          response.headers.get('Content-Disposition') ||
            response.headers.get('content-disposition') ||
            ''
        ) || `${item.name || 'dataset'}.jsonl`
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

  const removeDataset = async (item: any) => {
    try {
      await ElMessageBox.confirm('确认删除该评估基准？', '删除', { type: 'warning' })
      await evaluationApi.deleteDataset(item.dataset_id || item.id)
      ElMessage.success('已删除')
      await loadBenchmarks()
    } catch {
      // 取消
    }
  }

  const onCardCommand = (cmd: string, item: any) => {
    if (cmd === 'download') downloadDataset(item)
    if (cmd === 'delete') removeDataset(item)
  }

  watch(generateVisible, async (open) => {
    if (open) {
      generateForm.name = defaultName()
      await Promise.all([loadGraphStatus(), loadChatModels()])
    }
  })

  watch(
    () =>
      Object.values(tasker.tasks)
        .map((t) => `${t.id}:${t.status}`)
        .join('|'),
    () => {
      if (Object.values(tasker.tasks).some((t) => tasker.isTerminal(t.status))) loadBenchmarks(true)
    }
  )

  watch(() => props.kbId, () => loadBenchmarks(), { immediate: true })
  onBeforeUnmount(stopPoll)
</script>

<style scoped>
  .knowledge-benchmark-panel {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    box-sizing: border-box;
  }

  .panel-header {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 16px;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .total-count {
    font-size: 13px;
    color: var(--el-text-color-secondary);
  }

  .btn-icon {
    margin-right: 4px;
  }

  .panel-body {
    flex: 1 1 auto;
    min-height: 120px;
    overflow: auto;
  }

  .panel-loading {
    min-height: 160px;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    text-align: center;
    color: var(--el-text-color-secondary);
  }

  .empty-icon {
    font-size: 42px;
    color: var(--el-color-primary-light-5);
  }

  .empty-state h3 {
    margin: 0;
    font-size: 18px;
    color: var(--el-text-color-primary);
  }

  .empty-state p {
    margin: 0 0 8px;
    max-width: 360px;
    font-size: 13px;
  }

  .empty-actions {
    display: flex;
    gap: 8px;
  }

  .benchmark-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 12px;
    align-content: start;
    align-items: start;
  }

  .benchmark-card {
    height: auto;
    align-self: start;
    padding: 14px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
    cursor: pointer;
    transition: border-color 0.15s ease;
  }

  .benchmark-card:hover {
    border-color: var(--el-color-primary-light-5);
  }

  .benchmark-card.disabled {
    opacity: 0.75;
    cursor: default;
  }

  .card-top {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 8px;
  }

  .card-top h4 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .more-btn {
    border: none;
    background: transparent;
    color: var(--el-text-color-secondary);
    cursor: pointer;
    padding: 2px;
  }

  .card-desc {
    margin: 8px 0;
    min-height: 36px;
    font-size: 13px;
    color: var(--el-text-color-secondary);
    line-height: 1.4;
  }

  .card-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 12px;
  }

  .card-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .build-progress {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .upload-icon {
    font-size: 32px;
    color: var(--el-color-primary);
    margin-bottom: 2px;
  }

  .compact-form :deep(.el-form-item) {
    margin-bottom: 6px;
  }

  .compact-form :deep(.el-form-item__label) {
    margin-bottom: 1px !important;
    line-height: 1.2;
  }

  .compact-form .param-grid :deep(.el-form-item) {
    margin-bottom: 6px;
  }

  .benchmark-uploader {
    width: 100%;
  }

  .benchmark-uploader :deep(.el-upload) {
    width: 100%;
    display: block;
  }

  .benchmark-uploader :deep(.el-upload-dragger) {
    width: 100%;
    padding: 16px 12px;
  }

  .benchmark-uploader :deep(.el-upload__tip) {
    margin-top: 2px;
    line-height: 1.3;
  }

  .upload-file-item {
    margin-bottom: 0 !important;
  }

  .mode-cards-item :deep(.el-form-item__label) {
    margin-bottom: 0 !important;
  }

  .mode-cards {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    width: 100%;
  }

  .mode-card {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 4px;
    padding: 6px 10px 10px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 8px;
    background: var(--el-fill-color-blank);
    text-align: left;
    cursor: pointer;
  }

  .mode-card.active {
    border-color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);

    .mode-icon {
      color: var(--el-color-primary);
    }
  }

  .mode-card.disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .mode-card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
  }

  .mode-icon {
    flex-shrink: 0;
    font-size: 18px;
    color: var(--el-color-primary);
  }

  .mode-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .mode-tag {
    margin-left: auto;
    flex-shrink: 0;
  }

  .mode-description {
    margin: 0;
    font-size: 12px;
    color: var(--el-text-color-secondary);
    line-height: 1.4;
  }

  .mode-helper {
    margin: 0;
    font-size: 12px;
    color: var(--el-text-color-placeholder);
    line-height: 1.4;
  }

  .mode-helper.warning {
    color: var(--el-color-warning);
  }

  .param-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 0 10px;
  }

  .graph-expand-item {
    max-width: calc((100% - 20px) / 3);
  }

  .full {
    width: 100%;
  }

  .preview-title {
    margin: 0 0 4px;
    font-size: 16px;
    font-weight: 600;
  }

  .preview-desc {
    margin: 0;
    color: var(--el-text-color-secondary);
    font-size: 13px;
    line-height: 1.5;
  }

  .preview-panel {
    min-height: 320px;
  }

  .preview-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 12px;
    padding-bottom: 12px;
    border-bottom: 1px solid var(--el-border-color-lighter);
  }

  .preview-header-text {
    min-width: 0;
    flex: 1;
  }

  .preview-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 12px 16px;
    flex-shrink: 0;
    padding-top: 2px;
  }

  .meta-item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--el-text-color-regular);
  }

  .meta-label {
    color: var(--el-text-color-secondary);
  }

  .preview-table-section {
    min-height: 0;
  }

  .preview-table-title {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;
  }

  .preview-table-title h4 {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .preview-table-title span {
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .benchmark-preview-table-card {
    border: 1px solid var(--el-border-color-lighter);
  }

  .benchmark-preview-table-card :deep(.el-card__body) {
    padding: 8px 10px 10px;
  }

  .benchmark-preview-table-card :deep(.art-table .el-table) {
    margin-top: 0;
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

<style>
  .benchmark-preview-dialog.el-dialog {
    width: min(1080px, 94vw) !important;
  }

  .benchmark-preview-tooltip.el-popper {
    max-width: 360px !important;
  }

  .benchmark-preview-tooltip.el-popper .el-popper__arrow {
    display: none;
  }
</style>

