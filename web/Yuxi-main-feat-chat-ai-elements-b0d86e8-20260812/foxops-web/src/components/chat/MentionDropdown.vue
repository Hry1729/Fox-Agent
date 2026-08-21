<template>
  <div v-if="visible" ref="dropdownRef" class="mention-dropdown" :style="positionStyle" @mousedown.prevent>
    <div v-if="loading" class="mention-loading">搜索中...</div>
    <template v-else>
      <!-- 文件 -->
      <div v-if="items.files.length" class="mention-group">
        <div class="mention-group-title">文件</div>
        <div
          v-for="(item, idx) in items.files"
          :key="`file-${idx}`"
          class="mention-item"
          :class="{ active: selectedIndex === flatIndex('files', idx) }"
          @click="select(item)"
          @mouseenter="selectedIndex = flatIndex('files', idx)"
        >
          <ArtSvgIcon icon="ri:file-3-line" class="mention-icon" />
          <span class="mention-label">{{ item.label }}</span>
        </div>
      </div>
      <!-- 知识库 -->
      <div v-if="items.knowledgeBases.length" class="mention-group">
        <div class="mention-group-title">知识库</div>
        <div
          v-for="(item, idx) in items.knowledgeBases"
          :key="`kb-${idx}`"
          class="mention-item"
          :class="{ active: selectedIndex === flatIndex('knowledgeBases', idx) }"
          @click="select(item)"
          @mouseenter="selectedIndex = flatIndex('knowledgeBases', idx)"
        >
          <ArtSvgIcon icon="ri:book-2-line" class="mention-icon" />
          <span class="mention-label">{{ item.label }}</span>
        </div>
      </div>
      <!-- MCP -->
      <div v-if="items.mcps.length" class="mention-group">
        <div class="mention-group-title">MCP</div>
        <div
          v-for="(item, idx) in items.mcps"
          :key="`mcp-${idx}`"
          class="mention-item"
          :class="{ active: selectedIndex === flatIndex('mcps', idx) }"
          @click="select(item)"
          @mouseenter="selectedIndex = flatIndex('mcps', idx)"
        >
          <ArtSvgIcon icon="ri:plug-2-line" class="mention-icon" />
          <span class="mention-label">{{ item.label }}</span>
        </div>
      </div>
      <!-- Skills -->
      <div v-if="items.skills.length" class="mention-group">
        <div class="mention-group-title">Skills</div>
        <div
          v-for="(item, idx) in items.skills"
          :key="`skill-${idx}`"
          class="mention-item"
          :class="{ active: selectedIndex === flatIndex('skills', idx) }"
          @click="select(item)"
          @mouseenter="selectedIndex = flatIndex('skills', idx)"
        >
          <ArtSvgIcon icon="ri:tools-line" class="mention-icon" />
          <span class="mention-label">{{ item.label }}</span>
        </div>
      </div>
      <!-- Subagents -->
      <div v-if="items.subagents.length" class="mention-group">
        <div class="mention-group-title">子智能体</div>
        <div
          v-for="(item, idx) in items.subagents"
          :key="`sub-${idx}`"
          class="mention-item"
          :class="{ active: selectedIndex === flatIndex('subagents', idx) }"
          @click="select(item)"
          @mouseenter="selectedIndex = flatIndex('subagents', idx)"
        >
          <ArtSvgIcon icon="ri:robot-line" class="mention-icon" />
          <span class="mention-label">{{ item.label }}</span>
        </div>
      </div>
      <div v-if="!hasItems" class="mention-empty">暂无匹配项</div>
    </template>
  </div>
</template>

<script setup>
import { ref, computed, watch, nextTick } from 'vue'
import { mentionApi } from '@/api/mention'

const props = defineProps({
  visible: { type: Boolean, default: false },
  query: { type: String, default: '' },
  threadId: { type: String, default: '' },
  positionStyle: { type: Object, default: () => ({}) }
})

const emit = defineEmits(['select', 'close'])

const loading = ref(false)
const items = ref({ files: [], knowledgeBases: [], mcps: [], skills: [], subagents: [] })
const selectedIndex = ref(0)
const dropdownRef = ref(null)
let searchTimer = null
let searchRequestId = 0

const flatItems = computed(() => [
  ...items.value.files,
  ...items.value.knowledgeBases,
  ...items.value.mcps,
  ...items.value.skills,
  ...items.value.subagents
])

const hasItems = computed(() => flatItems.value.length > 0)

const flatIndex = (group, idx) => {
  let offset = 0
  for (const key of ['files', 'knowledgeBases', 'mcps', 'skills', 'subagents']) {
    if (key === group) return offset + idx
    offset += items.value[key]?.length || 0
  }
  return 0
}

const doSearch = async (query) => {
  const reqId = ++searchRequestId
  loading.value = true
  try {
    const result = await mentionApi.search(props.threadId, query)
    if (reqId !== searchRequestId) return // 过期请求
    items.value = {
      files: (result?.files || []).map((f) => ({ type: 'file', value: f.path, label: f.path?.split('/').pop() || f.path })),
      knowledgeBases: (result?.knowledgeBases || result?.knowledges || []).map((kb) => ({ type: 'knowledge', value: kb.kb_id || kb.id, label: kb.name || kb.kb_id })),
      mcps: (result?.mcps || []).map((m) => ({ type: 'mcp', value: m.slug || m.id, label: m.name || m.slug })),
      skills: (result?.skills || []).map((s) => ({ type: 'skill', value: s.slug || s.id, label: s.name || s.slug })),
      subagents: (result?.subagents || []).map((sa) => ({ type: 'subagent', value: sa.id || sa.slug, label: sa.name || sa.id }))
    }
    selectedIndex.value = 0
  } catch (e) {
    if (e?.name !== 'AbortError') console.warn('Mention search failed:', e)
  } finally {
    if (reqId === searchRequestId) loading.value = false
  }
}

watch(
  () => [props.visible, props.query],
  ([visible, query]) => {
    if (!visible) return
    clearTimeout(searchTimer)
    searchTimer = setTimeout(() => doSearch(query), 200)
  }
)

watch(
  () => props.visible,
  (visible) => {
    if (!visible) {
      items.value = { files: [], knowledgeBases: [], mcps: [], skills: [], subagents: [] }
      selectedIndex.value = 0
    }
  }
)

const select = (item) => {
  emit('select', item)
  emit('close')
}

const handleKeydown = (e) => {
  if (!props.visible) return false
  if (e.key === 'ArrowDown') {
    e.preventDefault()
    selectedIndex.value = Math.min(selectedIndex.value + 1, flatItems.value.length - 1)
    return true
  }
  if (e.key === 'ArrowUp') {
    e.preventDefault()
    selectedIndex.value = Math.max(selectedIndex.value - 1, 0)
    return true
  }
  if (e.key === 'Enter' || e.key === 'Tab') {
    e.preventDefault()
    const item = flatItems.value[selectedIndex.value]
    if (item) select(item)
    return true
  }
  if (e.key === 'Escape') {
    e.preventDefault()
    emit('close')
    return true
  }
  return false
}

defineExpose({ handleKeydown })
</script>

<style scoped>
.mention-dropdown {
  position: absolute;
  z-index: 100;
  min-width: 240px;
  max-width: 360px;
  max-height: 280px;
  overflow-y: auto;
  background: var(--el-bg-color);
  border: 1px solid var(--el-border-color-light);
  border-radius: 8px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.08);
}
.mention-loading,
.mention-empty {
  padding: 12px;
  text-align: center;
  font-size: 13px;
  color: var(--art-gray-400);
}
.mention-group {
  padding: 4px 0;
}
.mention-group + .mention-group {
  border-top: 1px solid var(--el-border-color-lighter);
}
.mention-group-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--art-gray-500);
  padding: 4px 12px 2px;
  text-transform: uppercase;
}
.mention-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 6px 12px;
  cursor: pointer;
  font-size: 13px;
  color: var(--el-text-color-primary);
  transition: background 0.1s;
}
.mention-item:hover,
.mention-item.active {
  background: var(--el-color-primary-light-9);
}
.mention-icon {
  font-size: 14px;
  color: var(--art-gray-500);
  flex-shrink: 0;
}
.mention-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
