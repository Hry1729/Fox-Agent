<template>
  <div v-if="hasContent" class="agent-state-panel">
    <!-- 上下文使用（复刻 Yuxi：百分比 + 当前 X/Y Token + 完整 token 构成 + 窗口/剩余） -->
    <div v-if="tokenUsage" class="state-section">
      <button class="state-section-header" @click="toggle('token')">
        <span class="state-section-label">
          <span class="state-title">上下文使用</span>
          <ArtSvgIcon icon="ri:arrow-down-s-line" class="state-chevron" :class="{ expanded: expanded.token }" />
        </span>
        <span class="state-meta">{{ tokenUsageHeaderPercentLabel }}</span>
      </button>
      <div v-if="expanded.token" class="state-section-body">
        <div class="token-usage-stack">
          <div class="token-usage-stack-head">
            <span>当前上下文</span>
            <strong>{{ tokenUsageStackHeadLabel }}</strong>
          </div>
          <div class="token-usage-stack-track" aria-label="Token 构成">
            <div
              v-for="segment in tokenUsageBarSegments"
              :key="segment.key"
              class="token-usage-stack-segment"
              :class="segment.tone"
              :style="{ width: segment.percent }"
              :title="`${segment.label}: ${segment.valueLabel}`"
            ></div>
          </div>
          <div class="token-usage-stack-legend">
            <span
              v-for="segment in tokenUsageSegments"
              :key="segment.key"
              class="token-usage-stack-legend-item"
            >
              <i :class="segment.tone"></i>
              {{ segment.label }} {{ segment.valueLabel }}
            </span>
          </div>
        </div>
        <div v-if="tokenUsageMetaRows.length" class="token-usage-breakdown">
          <div
            v-for="item in tokenUsageMetaRows"
            :key="item.key"
            class="token-usage-breakdown-row"
          >
            <span>{{ item.label }}</span>
            <strong>{{ item.value }}</strong>
          </div>
        </div>
      </div>
    </div>

    <!-- Todos -->
    <div v-if="todos.length" class="state-section">
      <button class="state-section-header" @click="toggle('todos')">
        <ArtSvgIcon icon="ri:checkbox-line" class="state-icon" />
        <span class="state-title">任务清单</span>
        <span class="state-meta">{{ completedTodoCount }}/{{ todos.length }}</span>
        <ArtSvgIcon icon="ri:arrow-down-s-line" class="state-chevron" :class="{ expanded: expanded.todos }" />
      </button>
      <div v-if="expanded.todos" class="state-section-body">
        <div v-for="(todo, idx) in todos" :key="idx" class="todo-item" :class="`status-${todo.status}`">
          <ArtSvgIcon
            :icon="todo.status === 'completed' ? 'ri:checkbox-circle-fill' : todo.status === 'in_progress' ? 'ri:radio-button-line' : 'ri:checkbox-blank-line'"
            class="todo-icon"
          />
          <span class="todo-content">{{ todo.content }}</span>
        </div>
      </div>
    </div>

    <!-- 文件 -->
    <div v-if="files.length" class="state-section">
      <button class="state-section-header" @click="toggle('files')">
        <ArtSvgIcon icon="ri:folder-line" class="state-icon" />
        <span class="state-title">文件</span>
        <span class="state-meta">{{ files.length }}</span>
        <ArtSvgIcon icon="ri:arrow-down-s-line" class="state-chevron" :class="{ expanded: expanded.files }" />
      </button>
      <div v-if="expanded.files" class="state-section-body">
        <div v-for="file in files" :key="file.key" class="state-list-item">
          <ArtSvgIcon icon="ri:file-3-line" class="state-file-icon" />
          <span class="state-file-name" :title="file.path">{{ file.name }}</span>
          <span v-if="file.sizeLabel" class="state-file-size">{{ file.sizeLabel }}</span>
        </div>
      </div>
    </div>

    <!-- 产物 -->
    <div v-if="artifacts.length" class="state-section">
      <button class="state-section-header" @click="toggle('artifacts')">
        <ArtSvgIcon icon="ri:gift-line" class="state-icon" />
        <span class="state-title">产物</span>
        <span class="state-meta">{{ artifacts.length }}</span>
        <ArtSvgIcon icon="ri:arrow-down-s-line" class="state-chevron" :class="{ expanded: expanded.artifacts }" />
      </button>
      <div v-if="expanded.artifacts" class="state-section-body">
        <div v-for="art in artifacts" :key="art.path" class="state-list-item artifact-item">
          <ArtSvgIcon icon="ri:file-download-line" class="state-file-icon" />
          <span class="state-file-name" :title="art.path">{{ art.name }}</span>
          <button v-if="art.path" class="artifact-download-btn" title="下载" @click.stop="downloadArtifact(art.path)">
            <ArtSvgIcon icon="ri:download-line" />
          </button>
        </div>
      </div>
    </div>

    <!-- 子智能体运行 -->
    <div v-if="subagentRuns.length" class="state-section">
      <button class="state-section-header" @click="toggle('subagents')">
        <ArtSvgIcon icon="ri:robot-line" class="state-icon" />
        <span class="state-title">子智能体</span>
        <span class="state-meta">{{ subagentRuns.length }}</span>
        <ArtSvgIcon icon="ri:arrow-down-s-line" class="state-chevron" :class="{ expanded: expanded.subagents }" />
      </button>
      <div v-if="expanded.subagents" class="state-section-body">
        <div v-for="run in subagentRuns" :key="run.id" class="subagent-item">
          <ArtSvgIcon icon="ri:robot-line" class="subagent-icon" />
          <div class="subagent-info">
            <span class="subagent-name">{{ run.name || run.agent_id || '子智能体' }}</span>
            <span class="subagent-status" :class="`run-status-${run.status}`">{{ run.status || '' }}</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { reactive, computed } from 'vue'
import { ElMessage } from 'element-plus'
import { threadApi } from '@/api/thread'

const props = defineProps({
  agentState: { type: Object, default: null },
  threadId: { type: String, default: '' }
})

const expanded = reactive({ token: true, todos: true, files: false, artifacts: false, subagents: false })
const toggle = (key) => { expanded[key] = !expanded[key] }

const TOKEN_K_UNIT = 1024
const toFiniteNumber = (v) => {
  const n = Number(v)
  return Number.isFinite(n) ? n : null
}
const formatToken = (value) => {
  const numeric = toFiniteNumber(value)
  if (numeric === null) return '-'
  if (numeric >= TOKEN_K_UNIT) {
    const digits = numeric >= TOKEN_K_UNIT * 10 ? 1 : 2
    return `${(numeric / TOKEN_K_UNIT).toFixed(digits).replace(/\.0+$/, '')}k`
  }
  return String(Math.round(numeric))
}
const toNum = (v) => toFiniteNumber(v) || 0

const tokenUsage = computed(() => props.agentState?.token_usage || null)

// Yuxi 风格的 token 段：包含已压缩、消息、摘要、系统、工具；超出部分计入"其他"
const tokenUsageSegments = computed(() => {
  const u = tokenUsage.value
  if (!u) return []
  const summaryTokens = u.summary_active ? Math.max(toNum(u.summary_message_tokens), 0) : 0
  const llmMessageTokens = Math.max(toNum(u.llm_messages_tokens), 0)
  const stateMessageTokensBeforeCall = Math.max(
    toNum(u.state_messages_tokens_before_call ?? u.state_messages_tokens),
    0
  )
  const messageTokens = Math.max(llmMessageTokens - summaryTokens, 0)
  const cutMessageTokens = Math.max(stateMessageTokensBeforeCall - llmMessageTokens, 0)
  const llmMessageCount = Math.max(toNum(u.llm_message_count), 0)
  const displayMessageCount = Math.max(llmMessageCount - (u.summary_active ? 1 : 0), 0)
  const stateMessageCountBeforeCall = Math.max(
    toNum(u.state_message_count_before_call ?? u.state_message_count),
    0
  )
  const cutMessageCount = Math.max(stateMessageCountBeforeCall - llmMessageCount, 0)
  const systemTokens = Math.max(toNum(u.system_tokens), 0)
  const toolsTokens = Math.max(toNum(u.tools_tokens), 0)
  const inputTokens = Math.max(toNum(u.llm_input_tokens), 0)

  const rawSegments = [
    {
      key: 'cut',
      label: '已压缩',
      value: cutMessageTokens,
      messageCount: cutMessageCount,
      tone: 'is-cut'
    },
    {
      key: 'messages',
      label: '消息',
      value: messageTokens,
      messageCount: displayMessageCount,
      tone: 'is-messages'
    },
    {
      key: 'summary',
      label: '摘要',
      value: summaryTokens,
      messageCount: u.summary_active ? 1 : 0,
      tone: 'is-summary'
    },
    {
      key: 'system',
      label: '系统',
      value: systemTokens,
      tone: 'is-system'
    },
    {
      key: 'tools',
      label: `工具 (${u.tool_count || 0})`,
      value: toolsTokens,
      tone: 'is-tools'
    }
  ].filter((s) => s.value > 0)

  const accountedInputTokens = llmMessageTokens + systemTokens + toolsTokens
  if (inputTokens > accountedInputTokens) {
    rawSegments.push({
      key: 'overhead',
      label: '其他',
      value: inputTokens - accountedInputTokens,
      tone: 'is-overhead'
    })
  }

  const segmentTotal = rawSegments.reduce((s, x) => s + x.value, 0)
  const total = Math.max(cutMessageTokens + inputTokens, segmentTotal, 1)
  return rawSegments.map((s) => {
    const ratio = s.value / total
    return {
      ...s,
      percent: `${Math.max(0, Math.min(ratio * 100, 100)).toFixed(2)}%`,
      valueLabel: s.messageCount
        ? `${formatToken(s.value)} (${s.messageCount}条)`
        : formatToken(s.value)
    }
  })
})

// 总用量 = llm_input_tokens（没有则用各段求和）
const tokenUsageStackTotal = computed(() => {
  const input = toFiniteNumber(tokenUsage.value?.llm_input_tokens)
  if (input !== null) return Math.max(input, 0)
  return tokenUsageSegments.value
    .filter((s) => s.key !== 'cut')
    .reduce((sum, s) => sum + s.value, 0)
})

// 上限 = summary_trigger_tokens（压缩阈值）> context_window > stackTotal
const tokenUsageStackLimit = computed(() => {
  const u = tokenUsage.value
  if (!u) return 1
  const summaryTrigger = toFiniteNumber(u.summary_trigger_tokens)
  if (summaryTrigger && summaryTrigger > 0) return summaryTrigger
  const ctxWindow = toFiniteNumber(u.context_window)
  if (ctxWindow && ctxWindow > 0) return ctxWindow
  return Math.max(tokenUsageStackTotal.value, 1)
})

// 标题百分比
const tokenUsageHeaderPercentLabel = computed(() => {
  const limit = Math.max(tokenUsageStackLimit.value, 1)
  const percent = Math.max(0, Math.min((tokenUsageStackTotal.value / limit) * 100, 100))
  if (percent > 0 && percent < 1) return '<1%'
  return `${Math.round(percent)}%`
})

// 头部右侧显示「X / Y Token」或「X Token」
const tokenUsageStackHeadLabel = computed(() => {
  const u = tokenUsage.value
  if (!u) return ''
  const summaryTrigger = toFiniteNumber(u.summary_trigger_tokens)
  if (summaryTrigger && summaryTrigger > 0) {
    return `${formatToken(tokenUsageStackTotal.value)} / ${formatToken(summaryTrigger)} Token`
  }
  return `${formatToken(tokenUsageStackTotal.value)} Token`
})

// 实际可视条：剔除 cut，按上限占比展示；剩余空间显示空白
const tokenUsageBarSegments = computed(() => {
  const limit = Math.max(tokenUsageStackLimit.value, 1)
  let remaining = limit
  return tokenUsageSegments.value
    .filter((s) => s.key !== 'cut')
    .map((s) => {
      const value = Math.min(s.value, Math.max(remaining, 0))
      remaining -= value
      return {
        ...s,
        percent: `${Math.max(0, Math.min((value / limit) * 100, 100)).toFixed(2)}%`
      }
    })
    .filter((s) => s.value > 0 && s.percent !== '0.00%')
})

// 额外元信息：窗口/剩余
const tokenUsageMetaRows = computed(() => {
  const u = tokenUsage.value
  if (!u) return []
  const rows = []
  const ctxWindow = toFiniteNumber(u.context_window)
  if (ctxWindow) {
    rows.push({
      key: 'context',
      label: '窗口 / 剩余',
      value: `${formatToken(ctxWindow)} / ${formatToken(u.remaining_context_tokens)}`
    })
  }
  return rows
})

const todos = computed(() => {
  const raw = props.agentState?.todos
  if (!Array.isArray(raw)) return []
  return raw.map((t) => ({
    content: String(t?.content || ''),
    status: t?.status || 'pending'
  }))
})
const completedTodoCount = computed(() => todos.value.filter((t) => t.status === 'completed').length)

const formatFileSize = (size) => {
  if (!size) return ''
  if (size < 1024) return `${size} B`
  if (size < 1048576) return `${(size / 1024).toFixed(1)} KB`
  return `${(size / 1048576).toFixed(1)} MB`
}

const files = computed(() => {
  const raw = props.agentState?.files
  const result = []
  const seen = new Set()
  if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
    Object.entries(raw).forEach(([path, data]) => {
      if (seen.has(path)) return
      seen.add(path)
      result.push({ key: path, path, name: path.split('/').pop() || path, sizeLabel: formatFileSize(data?.size || data?.file_size) })
    })
  }
  return result
})

const artifacts = computed(() => {
  const raw = props.agentState?.artifacts
  if (!Array.isArray(raw)) return []
  return raw.map((p) => String(p).trim()).filter(Boolean).map((path) => ({
    path,
    name: path.split('/').pop() || path
  }))
})

const subagentRuns = computed(() => {
  const raw = props.agentState?.subagent_runs
  if (!Array.isArray(raw)) return []
  return raw.map((r) => ({
    id: r?.id || r?.run_id || '',
    name: r?.name || r?.agent_id || '子智能体',
    status: r?.status || ''
  }))
})

const hasContent = computed(() =>
  Boolean(tokenUsage.value) ||
  todos.value.length > 0 ||
  files.value.length > 0 ||
  artifacts.value.length > 0 ||
  subagentRuns.value.length > 0
)

const downloadArtifact = async (path) => {
  if (!props.threadId || !path) return
  try {
    const blob = await threadApi.downloadThreadArtifact(props.threadId, path)
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = path.split('/').pop() || 'artifact'
    link.click()
    URL.revokeObjectURL(url)
  } catch (error) {
    ElMessage.error(error?.message || '下载失败')
  }
}
</script>

<style scoped>
.agent-state-panel {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 8px 0;
}
.state-section {
  border: 1px solid var(--art-gray-200);
  border-radius: 8px;
  overflow: hidden;
  background: var(--art-color);
}
.state-section-header {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 8px 12px;
  border: none;
  background: var(--art-gray-50);
  cursor: pointer;
  font-size: 13px;
  color: var(--el-text-color-primary);
  transition: background 0.15s;
}
.state-section-header:hover { background: var(--art-gray-100); }
.state-section-label {
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex: 1;
}
.state-icon { font-size: 14px; color: var(--art-gray-500); flex-shrink: 0; }
.state-title { font-weight: 500; }
.state-meta { font-size: 11px; color: var(--art-gray-500); flex-shrink: 0; font-variant-numeric: tabular-nums; }
.state-chevron { font-size: 14px; transition: transform 0.2s; color: var(--art-gray-500); }
.state-chevron.expanded { transform: rotate(180deg); }
.state-section-body { padding: 8px 12px; }

/* 上下文使用 - Yuxi 风格 stack */
.token-usage-content { display: flex; flex-direction: column; gap: 10px; padding-top: 2px; }
.token-usage-stack { display: flex; flex-direction: column; gap: 7px; }
.token-usage-stack-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  font-size: 11px;
  color: var(--art-gray-500);
}
.token-usage-stack-head strong {
  color: var(--art-gray-900);
  font-weight: 650;
  font-variant-numeric: tabular-nums;
}
.token-usage-stack-track {
  display: flex;
  height: 10px;
  border-radius: 999px;
  overflow: hidden;
  background: var(--art-gray-100);
}
.token-usage-stack-segment {
  height: 100%;
  min-width: 2px;
  transition: width 0.2s ease;
}
.token-usage-stack-legend {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 6px 8px;
  font-size: 11px;
  color: var(--art-gray-500);
}
.token-usage-stack-legend-item {
  min-width: 0;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.token-usage-stack-legend-item i {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 2px;
  background: var(--art-gray-300);
  display: inline-block;
}
.token-usage-stack-segment.is-cut,
.token-usage-stack-legend-item i.is-cut {
  background-color: var(--el-color-primary);
  background-image: repeating-linear-gradient(
    135deg,
    rgba(64, 158, 255, 0.3) 0,
    rgba(64, 158, 255, 0.3) 1px,
    transparent 1px,
    transparent 4px
  );
}
.token-usage-stack-segment.is-messages,
.token-usage-stack-legend-item i.is-messages { background: var(--el-color-primary); }
.token-usage-stack-segment.is-summary,
.token-usage-stack-legend-item i.is-summary { background: #8b5cf6; }
.token-usage-stack-segment.is-system,
.token-usage-stack-legend-item i.is-system { background: var(--el-color-success); }
.token-usage-stack-segment.is-tools,
.token-usage-stack-legend-item i.is-tools { background: var(--el-color-warning); }
.token-usage-stack-segment.is-overhead,
.token-usage-stack-legend-item i.is-overhead { background: var(--art-gray-300); }

/* 窗口 / 剩余 拆分 */
.token-usage-breakdown {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 6px 10px;
  padding-top: 2px;
}
.token-usage-breakdown-row {
  min-width: 0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  font-size: 12px;
  color: var(--art-gray-500);
}
.token-usage-breakdown-row span,
.token-usage-breakdown-row strong {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.token-usage-breakdown-row strong {
  color: var(--art-gray-800);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

/* Todos */
.todo-item { display: flex; align-items: center; gap: 6px; padding: 4px 0; font-size: 13px; }
.todo-icon { font-size: 14px; flex-shrink: 0; }
.todo-content { flex: 1; min-width: 0; }
.status-completed .todo-content { text-decoration: line-through; color: var(--art-gray-400); }
.status-completed .todo-icon { color: var(--el-color-success); }
.status-in_progress .todo-icon { color: var(--el-color-primary); }
.status-pending .todo-icon { color: var(--art-gray-300); }

/* Files / Artifacts */
.state-list-item { display: flex; align-items: center; gap: 6px; padding: 4px 0; font-size: 13px; }
.state-file-icon { font-size: 14px; color: var(--art-gray-500); flex-shrink: 0; }
.state-file-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.state-file-size { font-size: 11px; color: var(--art-gray-400); flex-shrink: 0; }
.artifact-download-btn { border: none; background: transparent; cursor: pointer; color: var(--art-gray-500); padding: 2px; }
.artifact-download-btn:hover { color: var(--el-color-primary); }

/* Subagents */
.subagent-item { display: flex; align-items: center; gap: 6px; padding: 4px 0; font-size: 13px; }
.subagent-icon { font-size: 14px; color: var(--art-gray-500); }
.subagent-info { display: flex; align-items: center; gap: 6px; flex: 1; }
.subagent-name { font-weight: 500; }
.subagent-status { font-size: 11px; padding: 1px 6px; border-radius: 4px; }
.run-status-running { background: var(--el-color-primary-light-9); color: var(--el-color-primary); }
.run-status-completed { background: var(--el-color-success-light-9); color: var(--el-color-success); }
.run-status-failed { background: var(--el-color-danger-light-9); color: var(--el-color-danger); }
</style>
