<template>
  <ElPopover
    ref="popoverRef"
    v-model:visible="dropdownOpen"
    placement="bottom-start"
    :width="320"
    trigger="manual"
    :show-arrow="false"
    :teleported="true"
  >
    <template #reference>
      <button
        ref="triggerRef"
        class="model-select"
        :class="{ 'model-select--disabled': disabled }"
        :disabled="disabled"
        @click="toggleDropdown"
      >
        <span class="model-text" :title="displayModelTitle">{{ displayModelText }}</span>
      </button>
    </template>

    <div class="model-dropdown">
      <ElInput
        v-model="modelSearchKeyword"
        placeholder="搜索模型"
        clearable
        size="small"
        class="model-search"
      >
        <template #suffix>
          <button
            :disabled="disabled || refreshingCache"
            :title="refreshingCache ? '刷新中...' : '刷新缓存'"
            class="cache-refresh-btn"
            @click.stop="refreshCache"
          >
            <ArtSvgIcon
              icon="ri:refresh-line"
              :class="{ spin: refreshingCache }"
              class="text-xs"
            />
          </button>
        </template>
      </ElInput>

      <div class="model-list">
        <div v-if="loadingModels" class="model-loading">加载中...</div>
        <div v-else-if="!hasFilteredModels" class="model-empty">暂无匹配模型</div>
        <template v-else>
          <div v-for="(providerData, providerId) in filteredModels" :key="providerId" class="provider-group">
            <div class="provider-title">{{ getProviderDisplayName(providerId, providerData) }}</div>
            <div
              v-for="model in providerData.models"
              :key="model.spec"
              class="model-item"
              :class="{ selected: model.spec === model_spec }"
              @click="handleSelectModel(model.spec)"
            >
              {{ model.display_name }}
            </div>
          </div>
        </template>
      </div>
    </div>
  </ElPopover>
</template>

<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import { modelProviderApi } from '@/api/model'

const props = defineProps({
  model_spec: { type: String, default: '' },
  placeholder: { type: String, default: '选择模型' },
  displayName: { type: String, default: 'full' },
  disabled: { type: Boolean, default: false }
})

const emit = defineEmits(['select-model'])

const popoverRef = ref(null)
const triggerRef = ref(null)
const dropdownOpen = ref(false)
const models = ref({})
const loadingModels = ref(false)
const modelSearchKeyword = ref('')
const refreshingCache = ref(false)
let fetchPromise = null

const filteredModels = computed(() => {
  const keyword = modelSearchKeyword.value.trim().toLowerCase()
  if (!keyword) return models.value

  return Object.entries(models.value).reduce((result, [providerId, providerData]) => {
    const matched = (providerData.models || []).filter((model) => {
      return [providerId, model.spec, model.model_id, model.display_name].some((v) =>
        String(v || '').toLowerCase().includes(keyword)
      )
    })
    if (matched.length) result[providerId] = { ...providerData, models: matched }
    return result
  }, {})
})

const hasFilteredModels = computed(() =>
  Object.values(filteredModels.value).some((p) => p.models?.length)
)

const getProviderDisplayName = (providerId, providerData = {}) =>
  providerData.provider_display_name || providerData.display_name || providerData.name || providerId

const extractModelName = (spec) => {
  const idx = spec.indexOf(':')
  return idx >= 0 ? spec.slice(idx + 1) : spec
}

const displayModelText = computed(() => {
  const spec = props.model_spec
  if (!spec) return props.placeholder
  const modelName = extractModelName(spec)
  if (props.displayName === 'mini') return modelName.includes('/') ? modelName.split('/').pop() : modelName
  if (props.displayName === 'short') return modelName
  return spec
})

const displayModelTitle = computed(() => props.model_spec || props.placeholder)

const fetchModels = async () => {
  if (fetchPromise) return fetchPromise
  loadingModels.value = true
  fetchPromise = (async () => {
    try {
      const response = await modelProviderApi.getV2Models('chat')
      if (response.success !== false) {
        models.value = response.data || response || {}
      }
    } catch (e) {
      console.warn('Failed to load models:', e)
    } finally {
      loadingModels.value = false
      fetchPromise = null
    }
  })()
  return fetchPromise
}

const toggleDropdown = () => {
  if (props.disabled) return
  dropdownOpen.value = !dropdownOpen.value
  if (dropdownOpen.value) fetchModels()
}

// 点击弹窗外部/按 ESC 关闭
const handleOutsideClick = (e) => {
  if (!dropdownOpen.value) return
  const popoverEl = popoverRef.value?.popperRef?.contentRef || popoverRef.value?.$el
  const triggerEl = triggerRef.value
  const target = e.target
  if (popoverEl?.contains?.(target)) return
  if (triggerEl?.contains?.(target)) return
  dropdownOpen.value = false
}

const handleKeydown = (e) => {
  if (e.key === 'Escape' && dropdownOpen.value) {
    dropdownOpen.value = false
  }
}

onMounted(() => {
  if (typeof document !== 'undefined') {
    document.addEventListener('mousedown', handleOutsideClick, true)
    document.addEventListener('keydown', handleKeydown)
  }
})

onBeforeUnmount(() => {
  if (typeof document !== 'undefined') {
    document.removeEventListener('mousedown', handleOutsideClick, true)
    document.removeEventListener('keydown', handleKeydown)
  }
})

const refreshCache = async () => {
  if (props.disabled || refreshingCache.value) return
  refreshingCache.value = true
  try {
    await modelProviderApi.refreshModelCache()
    await fetchModels()
  } catch (e) {
    console.error('Failed to refresh cache:', e)
  } finally {
    refreshingCache.value = false
  }
}

const handleSelectModel = (spec) => {
  if (props.disabled) return
  emit('select-model', spec)
  dropdownOpen.value = false
}
</script>

<style scoped>
.model-select {
  display: flex;
  align-items: center;
  min-height: 30px;
  padding: 0 8px;
  border: none;
  border-radius: 8px;
  font-size: 13px;
  color: var(--el-text-color-secondary);
  background: transparent;
  cursor: pointer;
  transition: all 0.2s;
  user-select: none;
}
.model-select:hover {
  background: var(--el-fill-color-light);
  color: var(--el-text-color-primary);
}
.model-select--disabled {
  cursor: not-allowed;
  opacity: 0.55;
}
.model-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  min-width: 0;
  text-align: left;
}
.model-dropdown {
  display: flex;
  flex-direction: column;
  max-height: 360px;
}
.model-search {
  margin-bottom: 8px;
}
.cache-refresh-btn {
  border: none;
  background: transparent;
  cursor: pointer;
  padding: 2px;
  display: flex;
  align-items: center;
  justify-content: center;
}
.cache-refresh-btn:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}
.spin {
  animation: spin 1s linear infinite;
}
@keyframes spin {
  to { transform: rotate(360deg); }
}
.model-list {
  max-height: 260px;
  overflow-y: auto;
}
.model-loading,
.model-empty {
  text-align: center;
  padding: 16px;
  color: var(--art-gray-400);
  font-size: 13px;
}
.provider-group {
  margin-bottom: 4px;
}
.provider-title {
  font-size: 11px;
  font-weight: 600;
  color: var(--art-gray-500);
  padding: 4px 8px;
  text-transform: uppercase;
}
.model-item {
  padding: 6px 12px;
  border-radius: 6px;
  font-size: 13px;
  cursor: pointer;
  transition: background 0.15s;
}
.model-item:hover {
  background: var(--art-gray-50);
}
.model-item.selected {
  color: var(--el-color-primary);
  background: var(--el-color-primary-light-9);
  font-weight: 500;
}
</style>
