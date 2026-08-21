<template>
  <div class="knowledge-query-panel">
    <div class="query-layout">
      <!-- 左侧：输入 + 示例/结果 -->
      <div class="query-main">
        <ElCard class="query-input-card" shadow="never">
          <ElInput
            v-model="query"
            type="textarea"
            :autosize="{ minRows: 2, maxRows: 6 }"
            placeholder="输入查询内容..."
            class="query-textarea"
            @keydown.enter.exact.prevent="runQuery"
          />
          <div class="query-actions">
            <span class="query-hint">Enter 检索知识库内容</span>
            <div class="query-action-btns">
              <ElTooltip :content="showRawData ? '切换至格式化显示' : '切换至原始数据'">
                <ElButton
                  circle
                  :type="showRawData ? 'primary' : 'default'"
                  :plain="!showRawData"
                  @click="showRawData = !showRawData"
                >
                  <ArtSvgIcon icon="ri:braces-line" />
                </ElButton>
              </ElTooltip>
              <ElButton
                type="primary"
                circle
                :loading="querying"
                :disabled="!query.trim()"
                @click="runQuery"
              >
                <ArtSvgIcon icon="ri:search-line" />
              </ElButton>
            </div>
          </div>
        </ElCard>

        <ElCard class="query-result-card" shadow="never" v-loading="querying">
          <template v-if="hasResults">
            <div v-if="showRawData" class="result-raw">
              <pre>{{ rawJson }}</pre>
            </div>
            <div v-else-if="typeof queryResult === 'string'" class="result-text">
              {{ queryResult }}
            </div>
            <div v-else-if="Array.isArray(results)" class="result-list">
              <div v-if="!results.length" class="no-results">未找到相关结果</div>
              <template v-else>
                <div class="result-summary">
                  <span>检索到 {{ results.length }} 个相关文档块</span>
                  <ElButton text type="primary" @click="clearResults">清空</ElButton>
                </div>
                <div v-for="(chunk, index) in results" :key="index" class="result-item">
                  <div class="result-header">
                    <span class="result-index">#{{ index + 1 }}</span>
                    <span v-if="chunk.score != null" class="result-score">
                      相似度: {{ formatPercent(chunk.score) }}
                    </span>
                    <span v-if="chunk.rerank_score != null" class="result-rerank">
                      重排序: {{ formatPercent(chunk.rerank_score) }}
                    </span>
                    <button
                      v-if="isContentLong(chunk)"
                      type="button"
                      class="expand-btn"
                      @click="toggleExpand(index)"
                    >
                      {{ expandedIndexes.has(index) ? '收起' : '展开' }}
                    </button>
                  </div>
                  <div
                    class="result-content"
                    :class="{ 'is-collapsed': !expandedIndexes.has(index) }"
                  >
                    {{ getChunkText(chunk) }}
                  </div>
                  <div class="result-metadata">
                    <span v-if="chunk.metadata?.source" class="meta-item">
                      <strong>来源:</strong> {{ chunk.metadata.source }}
                    </span>
                    <span v-if="chunk.metadata?.file_id" class="meta-item">
                      <strong>文件ID:</strong> {{ chunk.metadata.file_id }}
                    </span>
                    <span v-if="chunk.metadata?.chunk_index !== undefined" class="meta-item">
                      <strong>块索引:</strong> {{ chunk.metadata.chunk_index }}
                    </span>
                    <span v-if="chunk.distance !== undefined" class="meta-item">
                      <strong>距离:</strong> {{ Number(chunk.distance).toFixed(4) }}
                    </span>
                  </div>
                </div>
              </template>
            </div>
            <div v-else class="result-raw">
              <pre>{{ rawJson }}</pre>
            </div>
          </template>

          <div v-else class="query-suggestions">
            <div v-if="loadingQuestions || generating" class="suggestions-loading">
              <ElIcon class="is-loading"><Loading /></ElIcon>
              <span>{{ generating ? '正在生成示例问题...' : '正在加载示例问题...' }}</span>
            </div>
            <template v-else-if="visibleSamples.length">
              <div class="suggestions-title">示例问题</div>
              <button
                v-for="(example, index) in visibleSamples"
                :key="`${index}-${example}`"
                type="button"
                class="suggestion-row"
                @click="useSample(example)"
              >
                <ArtSvgIcon icon="ri:search-line" class="suggestion-icon" />
                <span class="suggestion-text">{{ example }}</span>
              </button>
              <button type="button" class="suggestion-row" @click="generateQuestions">
                <ArtSvgIcon icon="ri:refresh-line" class="suggestion-icon" />
                <span class="suggestion-text">重新生成</span>
              </button>
            </template>
            <div v-else class="suggestions-empty">
              <button type="button" class="suggestion-row" @click="generateQuestions">
                <ArtSvgIcon icon="ri:refresh-line" class="suggestion-icon" />
                <span class="suggestion-text">生成示例问题</span>
              </button>
            </div>
          </div>
        </ElCard>
      </div>

      <!-- 右侧：检索配置 -->
      <aside class="query-config-pane">
        <ElCard class="query-config-card" shadow="never">
          <div class="config-header">
            <div>
              <h3>检索配置</h3>
              <p>调整当前知识库的检索参数。</p>
            </div>
            <ElButton type="primary" size="small" :loading="configSaving" @click="saveConfig">
              <ArtSvgIcon icon="ri:save-line" class="mr-1" />
              保存
            </ElButton>
          </div>
          <div class="config-body">
            <KnowledgeSearchConfig
              ref="configRef"
              v-model="meta"
              :kb-id="kbId"
            />
          </div>
        </ElCard>
      </aside>
    </div>
  </div>
</template>

<script setup lang="ts">
  import { Loading } from '@element-plus/icons-vue'
  import { ElMessage } from 'element-plus'
  import { knowledgeApi } from '@/api/knowledge'
  import KnowledgeSearchConfig from './KnowledgeSearchConfig.vue'

  const props = defineProps<{ kbId: string }>()

  const MAX_VISIBLE_EXAMPLES = 10

  const query = ref('')
  const meta = ref<Record<string, unknown>>({})
  const querying = ref(false)
  const generating = ref(false)
  const loadingQuestions = ref(false)
  const showRawData = ref(false)
  const queryResult = ref<any>(null)
  const sampleQuestions = ref<string[]>([])
  const visibleSamples = ref<string[]>([])
  const configRef = ref<InstanceType<typeof KnowledgeSearchConfig> | null>(null)
  const configSaving = ref(false)
  /** 已展开全文的结果卡片索引 */
  const expandedIndexes = ref<Set<number>>(new Set())
  const CONTENT_COLLAPSE_LEN = 320

  const results = computed(() => {
    if (Array.isArray(queryResult.value)) return queryResult.value
    if (Array.isArray(queryResult.value?.results)) return queryResult.value.results
    if (Array.isArray(queryResult.value?.docs)) return queryResult.value.docs
    if (Array.isArray(queryResult.value?.items)) return queryResult.value.items
    return []
  })

  const hasResults = computed(() => queryResult.value !== null && queryResult.value !== '')
  const rawJson = computed(() => JSON.stringify(queryResult.value, null, 2))

  const formatPercent = (value: unknown) => {
    const num = Number(value)
    if (!Number.isFinite(num)) return '-'
    return `${(num * 100).toFixed(2)}%`
  }

  const getChunkText = (chunk: any) =>
    String(chunk?.content || chunk?.text || chunk?.page_content || '-').trim()

  const isContentLong = (chunk: any) => getChunkText(chunk).length > CONTENT_COLLAPSE_LEN

  const toggleExpand = (index: number) => {
    const next = new Set(expandedIndexes.value)
    if (next.has(index)) next.delete(index)
    else next.add(index)
    expandedIndexes.value = next
  }

  const normalizeQuestions = (payload: any): string[] => {
    const list = payload?.questions || payload?.data?.questions || payload || []
    if (!Array.isArray(list)) return []
    return list
      .map((item) => (typeof item === 'string' ? item : item?.question || item?.text || ''))
      .filter(Boolean)
  }

  const updateSamples = (questions: string[] = []) => {
    sampleQuestions.value = questions
    const shuffled = [...questions]
    for (let i = shuffled.length - 1; i > 0; i -= 1) {
      const j = Math.floor(Math.random() * (i + 1))
      ;[shuffled[i], shuffled[j]] = [shuffled[j], shuffled[i]]
    }
    visibleSamples.value = shuffled.slice(0, MAX_VISIBLE_EXAMPLES)
  }

  const loadSamples = async () => {
    if (!props.kbId) return
    loadingQuestions.value = true
    try {
      const result: any = await knowledgeApi.getSampleQuestions(props.kbId)
      updateSamples(normalizeQuestions(result))
    } catch {
      updateSamples([])
    } finally {
      loadingQuestions.value = false
    }
  }

  const generateQuestions = async () => {
    if (!props.kbId) return
    generating.value = true
    try {
      const result: any = await knowledgeApi.generateSampleQuestions(props.kbId, 10)
      const questions = normalizeQuestions(result)
      updateSamples(questions)
      if (questions.length) ElMessage.success(`成功生成 ${questions.length} 个测试问题`)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '生成失败')
    } finally {
      generating.value = false
    }
  }

  const useSample = (example: string) => {
    query.value = example
    runQuery()
  }

  const clearResults = () => {
    queryResult.value = null
    expandedIndexes.value = new Set()
  }

  const runQuery = async () => {
    if (!query.value.trim()) {
      ElMessage.warning('请输入查询内容')
      return
    }
    querying.value = true
    try {
      const result: any = await knowledgeApi.queryTest(props.kbId, query.value.trim(), {
        ...meta.value,
        include_distances: true
      })
      if (result?.status === 'failed') {
        throw new Error(result.message || '检索失败')
      }
      queryResult.value = result
      expandedIndexes.value = new Set()
    } catch (error) {
      queryResult.value = null
      expandedIndexes.value = new Set()
      ElMessage.error((error as Error)?.message || '检索失败')
    } finally {
      querying.value = false
    }
  }

  const saveConfig = async () => {
    configSaving.value = true
    try {
      await configRef.value?.save()
    } finally {
      configSaving.value = false
    }
  }

  watch(
    () => props.kbId,
    async () => {
      query.value = ''
      queryResult.value = null
      expandedIndexes.value = new Set()
      updateSamples([])
      await loadSamples()
    },
    { immediate: true }
  )
</script>

<style scoped>
  .knowledge-query-panel {
    height: 100%;
    min-height: 520px;
  }

  .query-layout {
    display: flex;
    gap: 12px;
    height: 100%;
    min-height: 520px;
  }

  .query-main {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .query-input-card,
  .query-result-card,
  .query-config-card {
    border-radius: 10px;
  }

  .query-input-card :deep(.el-card__body) {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px 12px;
  }

  .query-textarea :deep(.el-textarea__inner) {
    box-shadow: none;
    padding: 0;
    resize: none;
  }

  .query-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .query-hint {
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .query-action-btns {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .query-result-card {
    flex: 1;
    min-height: 0;
  }

  .query-result-card :deep(.el-card__body) {
    height: 100%;
    max-height: calc(100vh - 320px);
    overflow: auto;
    padding: 14px 16px;
  }

  .result-summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--el-color-primary-light-9);
    color: var(--el-text-color-primary);
    font-size: 13px;
    font-weight: 500;
  }

  .result-item {
    margin-bottom: 8px;
    padding: 8px 10px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 8px;
    background: var(--el-bg-color);
  }

  .result-item:last-child {
    margin-bottom: 0;
  }

  .result-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-bottom: 4px;
  }

  .result-index {
    font-weight: 600;
    color: var(--el-color-primary);
  }

  .result-score,
  .result-rerank {
    font-size: 12px;
    padding: 1px 7px;
    border-radius: 12px;
    background: var(--el-fill-color);
    color: var(--el-text-color-regular);
  }

  .result-rerank {
    background: var(--el-color-warning-light-9);
    color: var(--el-color-warning-dark-2);
  }

  .expand-btn {
    margin-left: auto;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--el-color-primary);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }

  .expand-btn:hover {
    opacity: 0.8;
  }

  .result-content {
    font-size: 13px;
    line-height: 1.5;
    word-break: break-word;
  }

  .result-content.is-collapsed {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 6;
    line-clamp: 6;
    white-space: normal;
  }

  .result-content:not(.is-collapsed) {
    white-space: pre-wrap;
  }

  .result-metadata {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 12px;
    margin-top: 4px;
    font-size: 12px;
    color: var(--el-text-color-regular);
  }

  .meta-item strong {
    margin-right: 4px;
    color: var(--el-text-color-secondary);
    font-weight: 500;
  }

  .result-raw pre,
  .result-text {
    margin: 0;
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .no-results,
  .suggestions-loading,
  .suggestions-empty {
    min-height: 120px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--el-text-color-secondary);
    font-size: 13px;
  }

  .query-suggestions {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
  }

  .suggestions-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--el-text-color-secondary);
  }

  .suggestion-row {
    max-width: 100%;
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 8px 14px;
    border: none;
    border-radius: 999px;
    background: var(--el-fill-color-blank);
    border: 1px solid var(--el-border-color-lighter);
    color: var(--el-text-color-primary);
    cursor: pointer;
    text-align: left;
    font: inherit;
  }

  .suggestion-row:hover {
    border-color: var(--el-color-primary-light-5);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.06);
  }

  .suggestion-icon {
    flex-shrink: 0;
    color: var(--el-color-primary);
  }

  .suggestion-text {
    font-size: 13px;
    line-height: 1.5;
    word-break: break-word;
  }

  .query-config-pane {
    width: 360px;
    flex: 0 0 360px;
    min-height: 0;
    display: flex;
  }

  .query-config-card {
    width: 100%;
    display: flex;
    flex-direction: column;
  }

  .query-config-card :deep(.el-card__body) {
    height: 100%;
    display: flex;
    flex-direction: column;
    padding: 0;
    overflow: hidden;
  }

  .config-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--el-border-color-lighter);
    flex-shrink: 0;
  }

  .config-header h3 {
    margin: 0 0 4px;
    font-size: 16px;
    font-weight: 600;
  }

  .config-header p {
    margin: 0;
    font-size: 13px;
    color: var(--el-text-color-secondary);
  }

  .config-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 12px 16px 16px;
  }

  .mr-1 {
    margin-right: 4px;
  }

  @media (max-width: 1100px) {
    .query-layout {
      flex-direction: column;
    }

    .query-config-pane {
      width: 100%;
      flex: none;
    }

    .query-result-card :deep(.el-card__body) {
      max-height: 420px;
    }
  }
</style>
