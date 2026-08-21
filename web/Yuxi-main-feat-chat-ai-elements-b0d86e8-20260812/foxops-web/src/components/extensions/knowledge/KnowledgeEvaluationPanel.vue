<template>
  <div class="knowledge-evaluation-panel" v-loading="loading && !runs.length && !datasets.length">
    <!-- 无可用基准 -->
    <div v-if="!loading && !completedDatasets.length" class="empty-state panel-body">
      <ArtSvgIcon icon="ri:bar-chart-2-line" class="empty-icon" />
      <h3>暂无可用评估基准</h3>
      <p>先上传或生成评估基准，再运行 RAG 评估。</p>
      <ElButton type="primary" @click="$emit('goto-benchmarks')">
        <ArtSvgIcon icon="ri:clipboard-line" class="btn-icon" />
        前往基准管理
      </ElButton>
    </div>

    <template v-else>
      <div class="panel-body">
        <!-- 最后一次评估 -->
        <section class="section-card">
          <div class="section-header">
            <h4>最后一次评估</h4>
            <ElButton type="primary" :disabled="!completedDatasets.length" @click="openStartDialog">
              <ArtSvgIcon icon="ri:play-circle-line" class="btn-icon" />
              开始评估
            </ElButton>
          </div>

          <div v-if="latestRun" class="latest-content">
            <div class="latest-main">
              <div class="score-ring" :class="scoreRingClass(latestRun)">
                {{ formatRingValue(latestRun) }}
              </div>
              <div class="latest-info">
                <div class="latest-title">
                  <span>{{ getRunName(latestRun) }}</span>
                  <ElTag size="small" :type="statusTagType(latestRun.status)">
                    {{ statusText(latestRun.status) }}
                  </ElTag>
                </div>
                <div class="latest-meta">
                  {{ getDatasetName(latestRun.dataset_id) }} ·
                  {{ formatDate(latestRun.started_at || latestRun.created_at) }} ·
                  <button
                    v-if="latestRun.status === 'completed'"
                    type="button"
                    class="run-link"
                    @click="viewRun(latestRun)"
                  >
                    Run {{ latestRun.run_id || latestRun.id }}
                  </button>
                  <span v-else>Run {{ latestRun.run_id || latestRun.id }}</span>
                </div>
                <div v-if="latestRun.status === 'running'" class="latest-progress">
                  <ElProgress :percentage="runProgress(latestRun)" :show-text="false" />
                  <span>{{ runProgressMessage(latestRun) }}</span>
                </div>
              </div>
            </div>
            <div class="metric-grid">
              <div class="metric-card">
                <span>Recall@10</span>
                <strong>{{ formatMetric(getRecall10(latestRun)) }}</strong>
              </div>
              <div class="metric-card">
                <span>耗时</span>
                <strong>{{ formatDuration(latestRun) }}</strong>
              </div>
              <div class="metric-card">
                <span>数据量</span>
                <strong>{{ formatItems(latestRun) }}</strong>
              </div>
              <div class="metric-card">
                <span>完成率</span>
                <strong>{{ formatCompletion(latestRun) }}</strong>
              </div>
            </div>
          </div>
          <div v-else class="latest-empty">暂无评估记录，开始评估后会在这里展示最近一次结果。</div>
        </section>

        <!-- 历史记录 -->
        <section class="section-card section-card-history">
          <div class="section-header">
            <h4>历史评估记录</h4>
            <ElButton text :loading="refreshing" @click="refresh(true)">
              <ArtSvgIcon icon="ri:refresh-line" class="btn-icon" />
              刷新
            </ElButton>
          </div>
          <ElTable :data="runs" size="small" row-key="run_id">
            <ElTableColumn label="评估名称" min-width="180" show-overflow-tooltip>
              <template #default="{ row }">{{ getRunName(row) }}</template>
            </ElTableColumn>
            <ElTableColumn label="评估基准" min-width="180" show-overflow-tooltip>
              <template #default="{ row }">{{ getDatasetName(row.dataset_id) }}</template>
            </ElTableColumn>
            <ElTableColumn label="数据量" width="92" align="center" header-align="center">
              <template #default="{ row }">{{ formatItems(row) }}</template>
            </ElTableColumn>
            <ElTableColumn label="耗时" width="100" align="center" header-align="center">
              <template #default="{ row }">{{ formatRunDuration(row) }}</template>
            </ElTableColumn>
            <ElTableColumn label="Recall@10" width="100" align="center" header-align="center">
              <template #default="{ row }">
                <ElTag v-if="isMetricReady(row)" size="small" :type="metricTagType(getRecall10(row))">
                  {{ formatMetric(getRecall10(row)) }}
                </ElTag>
                <span v-else>-</span>
              </template>
            </ElTableColumn>
            <ElTableColumn label="综合评分" width="100" align="center" header-align="center">
              <template #default="{ row }">
                <ElTag v-if="isMetricReady(row)" size="small" :type="metricTagType(row.overall_score)">
                  {{ formatMetric(row.overall_score) }}
                </ElTag>
                <span v-else>-</span>
              </template>
            </ElTableColumn>
            <ElTableColumn label="状态" width="86" align="center" header-align="center">
              <template #default="{ row }">
                <ElTag size="small" :type="statusTagType(row.status)">
                  {{ statusText(row.status) }}
                </ElTag>
              </template>
            </ElTableColumn>
            <ElTableColumn label="操作" width="148" fixed="right" align="center" header-align="center">
              <template #default="{ row }">
                <div class="row-actions">
                  <ElButton
                    v-if="row.status === 'completed'"
                    link
                    type="primary"
                    @click="viewRun(row)"
                  >
                    查看
                  </ElButton>
                  <ElButton link type="danger" @click="removeRun(row)">删除</ElButton>
                </div>
              </template>
            </ElTableColumn>
          </ElTable>
        </section>
      </div>
    </template>

    <!-- 开始评估弹窗 -->
    <ElDialog
      v-model="startVisible"
      width="520px"
      destroy-on-close
      :show-close="false"
      class="extension-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
      footer-class="extension-dialog-footer"
    >
      <template #header>
        <ExtensionDialogHeader title="配置本次评估" @close="startVisible = false" />
      </template>
      <ElForm label-position="top">
        <ElFormItem label="评估名称" required>
          <ElInput v-model="configForm.name" maxlength="100" show-word-limit />
        </ElFormItem>
        <ElFormItem label="评估基准" required>
          <div class="dataset-row">
            <ElSelect v-model="selectedDatasetId" placeholder="请选择评估基准" class="full">
              <ElOption
                v-for="item in completedDatasets"
                :key="item.dataset_id"
                :label="`${item.name} (${item.item_count ?? 0} 个问题)`"
                :value="item.dataset_id"
              />
            </ElSelect>
            <ElButton :loading="loading" @click="loadDatasets(true)">
              <ArtSvgIcon icon="ri:refresh-line" />
            </ElButton>
          </div>
        </ElFormItem>
        <ElFormItem :label="selectedDataset?.has_gold_answers ? '答案生成模型（可选）' : '答案生成模型（当前基准无需）'">
          <ElSelect
            v-model="configForm.answer_llm"
            filterable
            clearable
            placeholder="可选"
            class="full"
            :disabled="!selectedDataset?.has_gold_answers"
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
        <ElFormItem :label="selectedDataset?.has_gold_answers ? '答案评判模型（可选）' : '答案评判模型（当前基准无需）'">
          <ElSelect
            v-model="configForm.judge_llm"
            filterable
            clearable
            placeholder="可选"
            class="full"
            :disabled="!selectedDataset?.has_gold_answers"
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
        <p class="form-hint">{{ evaluationHint }}</p>
      </ElForm>
      <template #footer>
        <ElButton @click="startVisible = false">取消</ElButton>
        <ElButton type="primary" :loading="starting" :disabled="!selectedDataset" @click="startEvaluation">
          开始评估
        </ElButton>
      </template>
    </ElDialog>

    <!-- 结果详情 -->
    <ElDialog
      v-model="resultVisible"
      width="1200px"
      top="3vh"
      align-center
      destroy-on-close
      :show-close="false"
      class="extension-dialog evaluation-result-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
    >
      <template #header>
        <ExtensionDialogHeader
          :title="`评估结果 - ${getRunName(selectedRun)}`"
          @close="resultVisible = false"
        />
      </template>
      <div v-loading="resultLoading" class="result-panel">
        <template v-if="selectedRun">
          <div class="result-summary-bar">
            <div class="result-summary-items">
              <span class="summary-item" :title="selectedRun.run_id || selectedRun.id">
                运行 ID：{{ selectedRun.run_id || selectedRun.id }}
              </span>
              <span class="summary-item">
                状态：
                <ElTag size="small" :type="statusTagType(selectedRun.status)">
                  {{ statusText(selectedRun.status) }}
                </ElTag>
              </span>
              <span class="summary-item">
                总体评分：
                <ElTag
                  v-if="selectedRun.overall_score != null"
                  size="small"
                  :type="metricTagType(selectedRun.overall_score)"
                >
                  {{ formatMetric(selectedRun.overall_score) }}
                </ElTag>
                <span v-else>-</span>
              </span>
              <span class="summary-item">总问题数：{{ selectedRun.total_items ?? resultTotal }}</span>
              <span class="summary-item">完成数：{{ selectedRun.completed_items ?? 0 }}</span>
              <span class="summary-item">总耗时：{{ resultDurationText }}</span>
            </div>
            <ElButton size="small" @click="toggleErrorOnly">
              {{ errorOnly ? '显示全部' : '仅查看错误' }}
            </ElButton>
          </div>

          <div class="result-metrics-bar">
            <div class="result-metrics-items">
              <span class="result-count">
                {{
                  errorOnly
                    ? `仅显示错误结果，共 ${resultTotal} 条`
                    : `显示全部结果，共 ${resultTotal} 条`
                }}
              </span>
              <span
                v-for="(value, key) in resultRetrievalMetrics"
                :key="key"
                class="compact-metric"
              >
                {{ formatMetricTitle(String(key)) }}：
                <strong :style="{ color: metricColor(value) }">{{ formatMetric(value) }}</strong>
              </span>
              <span v-if="resultAnswerAccuracy != null" class="compact-metric">
                答案准确率：
                <strong :style="{ color: metricColor(resultAnswerAccuracy) }">
                  {{ formatMetric(resultAnswerAccuracy) }}
                </strong>
              </span>
            </div>
          </div>

          <ElCard
            v-if="runResults.length"
            class="art-table-card evaluation-result-table-card"
            shadow="never"
          >
            <ArtTable
              :loading="resultLoading"
              :data="runResults"
              :columns="resultColumns"
              :pagination="resultPagination"
              :pagination-options="resultPaginationOptions"
              :height="resultTableHeight"
              :show-table-header="false"
              empty-text="暂无结果"
              empty-height="240px"
              size="small"
              @pagination:size-change="handleResultSizeChange"
              @pagination:current-change="handleResultCurrentChange"
            >
              <template #query="{ row }">
                <BenchmarkPreviewCell :text="getResultQuestion(row)" />
              </template>
              <template #generated_answer="{ row }">
                <BenchmarkPreviewCell :text="getResultAnswer(row)" placeholder="-" />
              </template>
              <template #retrieval_score="{ row }">
                <BenchmarkPreviewCell :text="formatRetrievalMetrics(row)" placeholder="-" />
              </template>
              <template #answer_score="{ row }">
                <div v-if="hasAnswerJudgement(row)" class="answer-judgement">
                  <ElTag size="small" :type="getAnswerJudgementType(row)">
                    {{ getAnswerJudgementLabel(row) }}
                  </ElTag>
                  <BenchmarkPreviewCell
                    v-if="getAnswerReasoning(row)"
                    :text="getAnswerReasoning(row)"
                    placeholder=""
                  />
                </div>
                <span v-else>-</span>
              </template>
            </ArtTable>
          </ElCard>
          <div v-else class="result-empty">暂无详细结果数据</div>
        </template>
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
  import { modelProviderApi } from '@/api/model'
  import { useTaskerStore } from '@/store/modules/tasker'
  import { unwrapApiData, unwrapList } from '@/utils/apiData'

  const props = defineProps<{ kbId: string }>()
  defineEmits<{ 'goto-benchmarks': [] }>()
  const tasker = useTaskerStore()

  const loading = ref(false)
  const refreshing = ref(false)
  const starting = ref(false)
  const resultLoading = ref(false)
  const chatModelsLoading = ref(false)
  const chatModels = ref<
    Record<string, { provider_display_name?: string; models: Array<{ spec: string; display_name?: string }> }>
  >({})
  const datasets = ref<any[]>([])
  const runs = ref<any[]>([])
  const startVisible = ref(false)
  const resultVisible = ref(false)
  const selectedDatasetId = ref('')
  const selectedRun = ref<any>(null)
  const runResults = ref<any[]>([])
  const errorOnly = ref(false)
  const resultPage = ref(1)
  const resultPageSize = ref(10)
  const resultTotal = ref(0)
  const resultRetrievalMetrics = ref<Record<string, number>>({})
  const resultAnswerAccuracy = ref<number | null>(null)
  // small 表格：表头 40px + 每行 48px，与下方 CSS 固定行高保持一致
  const RESULT_TABLE_HEADER_HEIGHT = 40
  const RESULT_TABLE_ROW_HEIGHT = 48
  const resultTableHeight = computed(
    () => RESULT_TABLE_HEADER_HEIGHT + RESULT_TABLE_ROW_HEIGHT * resultPageSize.value
  )
  const resultPaginationOptions = {
    pageSizes: [10, 20, 30, 50]
  }
  let pollTimer: ReturnType<typeof setInterval> | null = null

  const configForm = reactive({
    name: '',
    answer_llm: '',
    judge_llm: ''
  })

  const completedDatasets = computed(() =>
    datasets.value.filter((item) => (item.build_metadata?.status || 'completed') === 'completed')
  )
  const selectedDataset = computed(() =>
    completedDatasets.value.find((item) => item.dataset_id === selectedDatasetId.value)
  )
  const latestRun = computed(() => runs.value[0] || null)

  const resultPagination = computed(() => ({
    current: resultPage.value,
    size: resultPageSize.value,
    total: resultTotal.value
  }))

  const showRetrievalColumn = computed(() =>
    runResults.value.some((item) => hasRetrievalMetrics(item?.metrics))
  )

  const resultColumns = computed<ColumnOption[]>(() => {
    const columns: ColumnOption[] = [
      { prop: 'query', label: '问题', minWidth: 220, useSlot: true },
      { prop: 'generated_answer', label: '生成答案', minWidth: 280, useSlot: true }
    ]
    if (showRetrievalColumn.value) {
      columns.push({
        prop: 'retrieval_score',
        label: '检索指标',
        minWidth: 180,
        useSlot: true
      })
    }
    columns.push({ prop: 'answer_score', label: '答案评判', minWidth: 320, useSlot: true })
    return columns
  })

  const resultDurationText = computed(() => {
    const row = selectedRun.value
    if (!row?.started_at || !row?.completed_at) return '-'
    const duration =
      (new Date(row.completed_at).getTime() - new Date(row.started_at).getTime()) / 1000
    if (!Number.isFinite(duration) || duration < 0) return '-'
    if (duration < 60) return `${Math.round(duration)}s`
    return `${Math.floor(duration / 60)}m ${Math.round(duration % 60)}s`
  })

  const evaluationHint = computed(() => {
    if (!selectedDataset.value) return '请先选择评估基准。'
    if (selectedDataset.value.has_gold_answers) {
      return '若填写答案模型，则生成模型与评判模型需同时选择或同时留空。'
    }
    return '当前基准主要用于检索评估，可不配置答案模型。'
  })

  const defaultEvalName = () => {
    const today = new Date().toISOString().slice(0, 10)
    return `Eval-${today}-${Math.random().toString(36).slice(2, 6)}`
  }

  const statusText = (status?: string) => {
    const map: Record<string, string> = {
      pending: '排队中',
      running: '运行中',
      completed: '已完成',
      failed: '失败',
      cancelled: '已取消'
    }
    return map[status || ''] || status || '-'
  }

  const statusTagType = (status?: string) => {
    if (status === 'completed') return 'success'
    if (status === 'failed') return 'danger'
    if (status === 'running' || status === 'pending') return 'warning'
    return 'info'
  }

  const formatDate = (value?: string) => {
    if (!value) return '-'
    const d = new Date(value)
    return Number.isNaN(d.getTime()) ? String(value) : d.toLocaleString()
  }

  const getRecall10 = (row: any) => row?.metrics?.['recall@10'] ?? row?.metrics?.recall_at_10
  const formatMetric = (value: any) => {
    if (value == null || value === '') return '-'
    const n = Number(value)
    if (!Number.isFinite(n)) return String(value)
    return `${(n <= 1 ? n * 100 : n).toFixed(1)}%`
  }

  const getRunName = (row: any) => row?.name || row?.run_name || `Run ${row?.run_id || row?.id || ''}`
  const getDatasetName = (datasetId?: string) => {
    const found = datasets.value.find((item) => item.dataset_id === datasetId)
    return found?.name || datasetId || '-'
  }

  const formatRingValue = (row: any) => {
    if (row?.status === 'running') return `${runProgress(row)}%`
    return formatMetric(row?.overall_score)
  }

  const scoreRingClass = (row: any) => {
    if (row?.status === 'running') return 'is-running'
    const score = Number(row?.overall_score)
    if (!Number.isFinite(score)) return ''
    const pct = score <= 1 ? score * 100 : score
    if (pct >= 80) return 'is-good'
    if (pct >= 60) return 'is-mid'
    return 'is-low'
  }

  const runProgress = (row: any) => {
    const progress = Number(row?.progress)
    if (Number.isFinite(progress)) {
      return Math.max(0, Math.min(Math.round(progress), 100))
    }

    const total = Number(row?.total_items || 0)
    if (!total) return 0
    const done = Number(row?.completed_items || 0)
    return Math.max(0, Math.min(Math.round((done / total) * 100), 100))
  }

  const runProgressMessage = (row: any) => {
    if (row?.message) return row.message
    const total = Number(row?.total_items || 0)
    if (total) return `评估 ${Number(row?.completed_items || 0)}/${total}`
    return '评估进行中'
  }

  const formatDuration = (row: any) => {
    const seconds = Number(row?.duration_seconds ?? row?.duration)
    if (!Number.isFinite(seconds) || seconds < 0) return '-'
    if (seconds < 60) return `${Math.round(seconds)}s`
    return `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`
  }

  const formatRunDuration = (row: any) => {
    if (row?.status === 'running') return '进行中'
    const seconds = Number(row?.duration_seconds ?? row?.duration)
    if (Number.isFinite(seconds) && seconds >= 0) return formatDuration(row)
    if (!row?.started_at || !row?.completed_at) return '-'
    const duration =
      (new Date(row.completed_at).getTime() - new Date(row.started_at).getTime()) / 1000
    if (!Number.isFinite(duration) || duration < 0) return '-'
    if (duration < 60) return `${Math.round(duration)}s`
    return `${Math.floor(duration / 60)}m ${Math.round(duration % 60)}s`
  }

  const formatItems = (row: any) => `${row?.completed_items ?? 0}/${row?.total_items ?? 0}`

  const isMetricReady = (row: any) => row?.status === 'completed'

  const metricTagType = (value: any) => {
    const n = Number(value)
    if (!Number.isFinite(n)) return 'info'
    const pct = n <= 1 ? n * 100 : n
    if (pct >= 80) return 'success'
    if (pct >= 60) return 'warning'
    return 'danger'
  }

  const metricColor = (value: any) => {
    const n = Number(value)
    if (!Number.isFinite(n)) return 'var(--el-text-color-secondary)'
    const pct = n <= 1 ? n * 100 : n
    if (pct >= 80) return 'var(--el-color-success)'
    if (pct >= 60) return 'var(--el-color-warning)'
    return 'var(--el-color-danger)'
  }

  const formatMetricTitle = (key: string) => {
    const map: Record<string, string> = {
      'recall@10': 'Recall@10',
      'recall@5': 'Recall@5',
      map: 'MAP',
      ndcg: 'NDCG'
    }
    return map[key] || key
  }

  const hasRetrievalMetrics = (metrics?: Record<string, unknown>) => {
    if (!metrics) return false
    return Object.keys(metrics).some(
      (key) =>
        key.startsWith('recall') || key.startsWith('precision') || key === 'map' || key === 'ndcg'
    )
  }

  const getResultQuestion = (row: any) => String(row?.query || row?.question || '').trim()

  const getResultAnswer = (row: any) => String(row?.generated_answer || row?.answer || '').trim()

  const getAnswerScore = (row: any) => {
    const score = row?.metrics?.score
    return score == null ? null : Number(score)
  }

  const hasAnswerJudgement = (row: any) => row?.metrics?.score !== undefined

  const getAnswerJudgementLabel = (row: any) => {
    const score = Number(row?.metrics?.score)
    if (!Number.isFinite(score)) return '-'
    return score === 1 ? '正确' : '错误'
  }

  const getAnswerJudgementType = (row: any) => {
    const score = Number(row?.metrics?.score)
    if (!Number.isFinite(score)) return 'info'
    return score > 0.5 ? 'success' : 'danger'
  }

  const getAnswerReasoning = (row: any) => String(row?.metrics?.reasoning || '').trim()

  const formatRetrievalMetrics = (row: any) => {
    const metrics = row?.metrics || {}
    const parts = Object.entries(metrics)
      .filter(
        ([key]) =>
          key.startsWith('recall') ||
          key.startsWith('precision') ||
          key === 'map' ||
          key === 'ndcg'
      )
      .map(([key, value]) => `${formatMetricTitle(key)} ${formatMetric(value)}`)
    return parts.join(' · ')
  }

  const calculateResultStats = (items: any[]) => {
    const retrievalMetrics: Record<string, number> = {}
    const metricSums: Record<string, number> = {}
    const metricCounts: Record<string, number> = {}
    let correctAnswers = 0
    let scoredAnswers = 0

    items.forEach((item) => {
      const metrics = item?.metrics || {}
      Object.entries(metrics).forEach(([key, value]) => {
        if (
          key.startsWith('recall') ||
          key.startsWith('precision') ||
          key === 'map' ||
          key === 'ndcg'
        ) {
          const num = Number(value)
          if (!Number.isFinite(num)) return
          metricSums[key] = (metricSums[key] || 0) + num
          metricCounts[key] = (metricCounts[key] || 0) + 1
        }
      })
      if (metrics.score != null) {
        scoredAnswers += 1
        if (Number(metrics.score) > 0.5) correctAnswers += 1
      }
    })

    Object.keys(metricSums).forEach((key) => {
      retrievalMetrics[key] = metricSums[key] / (metricCounts[key] || 1)
    })

    resultRetrievalMetrics.value = retrievalMetrics
    resultAnswerAccuracy.value = scoredAnswers > 0 ? correctAnswers / scoredAnswers : null
  }

  const formatCompletion = (row: any) => {
    const total = Number(row?.total_items || 0)
    const done = Number(row?.completed_items || 0)
    if (!total) return '-'
    return `${((done / total) * 100).toFixed(0)}%`
  }

  const stopPoll = () => {
    if (pollTimer) {
      clearInterval(pollTimer)
      pollTimer = null
    }
  }

  const syncPoll = () => {
    stopPoll()
    if (runs.value.some((r) => r.status === 'running' || r.status === 'pending')) {
      pollTimer = setInterval(() => refresh(true), 4000)
    }
  }

  const loadDatasets = async (silent = false) => {
    try {
      const result: any = await evaluationApi.listDatasets(props.kbId)
      datasets.value = unwrapList(result).map((item: any) => ({
        ...item,
        dataset_id: item.dataset_id || item.id
      }))
      if (!selectedDatasetId.value && completedDatasets.value.length) {
        selectedDatasetId.value = completedDatasets.value[0].dataset_id
      }
    } catch (error) {
      if (!silent) ElMessage.error((error as Error)?.message || '加载评估基准失败')
    }
  }

  const loadRuns = async () => {
    const result: any = await evaluationApi.listRuns(props.kbId)
    runs.value = unwrapList(result).map((item: any) => ({
      ...item,
      run_id: item.run_id || item.id
    }))
    syncPoll()
  }

  const refresh = async (silent = false) => {
    if (silent) refreshing.value = true
    else loading.value = true
    try {
      await Promise.all([loadDatasets(true), loadRuns()])
    } catch (error) {
      if (!silent) ElMessage.error((error as Error)?.message || '加载评估数据失败')
    } finally {
      loading.value = false
      refreshing.value = false
    }
  }

  const openStartDialog = async () => {
    configForm.name = defaultEvalName()
    configForm.answer_llm = ''
    configForm.judge_llm = ''
    if (!selectedDatasetId.value && completedDatasets.value.length) {
      selectedDatasetId.value = completedDatasets.value[0].dataset_id
    }
    startVisible.value = true
    await loadChatModels()
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
    } catch {
      chatModels.value = {}
    } finally {
      chatModelsLoading.value = false
    }
  }

  const startEvaluation = async () => {
    if (!selectedDataset.value) return ElMessage.warning('请先选择评估基准')
    if (!configForm.name.trim()) return ElMessage.warning('请输入评估名称')
    const answer = selectedDataset.value.has_gold_answers ? configForm.answer_llm : ''
    const judge = selectedDataset.value.has_gold_answers ? configForm.judge_llm : ''
    if (Boolean(answer) !== Boolean(judge)) {
      return ElMessage.warning('生成模型和评估模型必须同时选择或同时不选择')
    }
    starting.value = true
    try {
      const result: any = await evaluationApi.runEvaluation(props.kbId, {
        dataset_id: selectedDataset.value.dataset_id,
        name: configForm.name.trim(),
        model_config: { answer_llm: answer, judge_llm: judge }
      })
      const taskId = result?.task_id || result?.id
      if (taskId) await tasker.trackTask(String(taskId))
      ElMessage.success('评估任务已开始')
      startVisible.value = false
      await refresh(true)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '启动评估失败')
    } finally {
      starting.value = false
    }
  }

  const viewRun = async (row: any) => {
    selectedRun.value = row
    errorOnly.value = false
    resultPage.value = 1
    resultVisible.value = true
    await loadResultPage()
  }

  const loadResultPage = async () => {
    if (!selectedRun.value) return
    resultLoading.value = true
    try {
      const runId = selectedRun.value.run_id || selectedRun.value.id
      const result: any = await evaluationApi.getRunResults(props.kbId, runId, {
        page: resultPage.value,
        pageSize: resultPageSize.value,
        errorOnly: errorOnly.value
      })
      const data = unwrapApiData(result, result)
      runResults.value = Array.isArray(data?.items) ? data.items : unwrapList(result)
      resultTotal.value = Number(data?.pagination?.total ?? runResults.value.length)
      selectedRun.value = {
        ...selectedRun.value,
        ...data,
        run_id: data?.run_id || runId
      }
      if (resultPage.value === 1 && !errorOnly.value) {
        calculateResultStats(runResults.value)
      }
    } catch (error) {
      runResults.value = []
      resultTotal.value = 0
      ElMessage.error((error as Error)?.message || '加载结果失败')
    } finally {
      resultLoading.value = false
    }
  }

  const toggleErrorOnly = async () => {
    errorOnly.value = !errorOnly.value
    resultPage.value = 1
    await loadResultPage()
  }

  const handleResultSizeChange = (size: number) => {
    resultPageSize.value = size
    resultPage.value = 1
    loadResultPage()
  }

  const handleResultCurrentChange = (page: number) => {
    resultPage.value = page
    loadResultPage()
  }

  const removeRun = async (row: any) => {
    try {
      await ElMessageBox.confirm('确认删除该评估记录？', '删除', { type: 'warning' })
      await evaluationApi.deleteRun(props.kbId, row.run_id || row.id)
      ElMessage.success('已删除')
      await refresh(true)
    } catch {
      // 取消
    }
  }

  watch(
    () =>
      Object.values(tasker.tasks)
        .map((t) => `${t.id}:${t.status}`)
        .join('|'),
    () => {
      if (Object.values(tasker.tasks).some((t) => tasker.isTerminal(t.status))) refresh(true)
    }
  )

  watch(() => props.kbId, () => refresh(), { immediate: true })
  onBeforeUnmount(stopPoll)
</script>

<style scoped>
  .knowledge-evaluation-panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    box-sizing: border-box;
  }

  .panel-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .btn-icon {
    margin-right: 4px;
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

  .empty-state h3,
  .section-header h4 {
    margin: 0;
    color: var(--el-text-color-primary);
  }

  .empty-state h3 {
    font-size: 18px;
  }

  .empty-state p {
    margin: 0 0 8px;
    max-width: 360px;
    font-size: 13px;
  }

  .section-card {
    flex-shrink: 0;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    padding: 16px;
    background: var(--el-bg-color);
  }

  .section-card-history {
    flex: 1;
    min-height: 280px;
    display: flex;
    flex-direction: column;
  }

  .section-card-history :deep(.row-actions) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 2px;
    white-space: nowrap;
    width: 100%;
  }

  .section-card-history :deep(.row-actions .el-button) {
    padding: 0 4px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
  }

  .section-header h4 {
    font-size: 15px;
    font-weight: 600;
  }

  .latest-content {
    min-height: 0;
  }

  .latest-main {
    display: flex;
    gap: 16px;
    align-items: center;
    margin-bottom: 14px;
  }

  .score-ring {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 14px;
    font-weight: 700;
    border: 3px solid var(--el-border-color);
    flex-shrink: 0;
  }

  .score-ring.is-good {
    border-color: var(--el-color-success);
    color: var(--el-color-success);
  }

  .score-ring.is-mid {
    border-color: var(--el-color-warning);
    color: var(--el-color-warning);
  }

  .score-ring.is-low {
    border-color: var(--el-color-danger);
    color: var(--el-color-danger);
  }

  .score-ring.is-running {
    border-color: var(--el-color-primary);
    color: var(--el-color-primary);
  }

  .latest-title {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 15px;
    font-weight: 600;
    margin-bottom: 6px;
  }

  .latest-meta {
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .run-link {
    border: none;
    background: transparent;
    color: var(--el-color-primary);
    cursor: pointer;
    padding: 0;
    font: inherit;
  }

  .latest-progress {
    margin-top: 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .metric-grid {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 10px;
  }

  .metric-card {
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--el-fill-color-lighter);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .metric-card span {
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .metric-card strong {
    font-size: 16px;
  }

  .latest-empty {
    padding: 24px 16px;
    border: 1px dashed var(--el-border-color-lighter);
    border-radius: 8px;
    text-align: center;
    color: var(--el-text-color-secondary);
    font-size: 13px;
  }

  .dataset-row {
    display: flex;
    gap: 8px;
    width: 100%;
  }

  .full {
    width: 100%;
  }

  .form-hint {
    margin: 0;
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .result-panel {
    min-height: 360px;
  }

  .result-summary-bar {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 10px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--el-border-color-lighter);
  }

  .result-summary-items {
    display: flex;
    flex-wrap: wrap;
    gap: 10px 16px;
    font-size: 13px;
    color: var(--el-text-color-regular);
  }

  .summary-item {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .result-metrics-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 10px;
  }

  .result-metrics-items {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 14px;
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .result-count {
    color: var(--el-text-color-regular);
  }

  .compact-metric strong {
    font-weight: 600;
  }

  .evaluation-result-table-card {
    border: 1px solid var(--el-border-color-lighter);
  }

  .evaluation-result-table-card :deep(.el-card__body) {
    padding: 8px 10px 10px;
  }

  .evaluation-result-table-card :deep(.art-table .el-table) {
    margin-top: 0;
  }

  .evaluation-result-table-card :deep(.el-table--small) {
    --result-row-height: 48px;
  }

  .evaluation-result-table-card :deep(.el-table--small .el-table__body tr) {
    height: var(--result-row-height);
  }

  .evaluation-result-table-card :deep(.el-table--small .el-table__body td.el-table__cell) {
    padding: 6px 0;
  }

  .evaluation-result-table-card :deep(.el-table--small .el-table__body .cell) {
    line-height: 20px;
  }

  .answer-judgement {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    width: 100%;
    min-width: 0;
  }

  .answer-judgement :deep(.el-tag) {
    flex-shrink: 0;
    margin-top: 2px;
  }

  .answer-judgement :deep(.benchmark-preview-cell) {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 12px;
    color: var(--el-text-color-secondary);
    line-height: 1.45;
  }

  .result-empty {
    padding: 48px 16px;
    text-align: center;
    color: var(--el-text-color-secondary);
    font-size: 13px;
    border: 1px dashed var(--el-border-color-lighter);
    border-radius: 8px;
  }

  @media (max-width: 900px) {
    .metric-grid {
      grid-template-columns: repeat(2, 1fr);
    }
  }
</style>

<style>
  .evaluation-result-dialog.el-dialog {
    width: min(1200px, 96vw) !important;
  }
</style>
