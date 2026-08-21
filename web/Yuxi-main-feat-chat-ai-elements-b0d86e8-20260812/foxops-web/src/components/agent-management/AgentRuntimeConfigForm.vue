<template>
  <div class="agent-runtime-config-form">
    <div class="runtime-config-content">
      <ElAlert v-if="isEmptyConfig" type="warning" show-icon :closable="false" class="config-alert">
        该智能体没有配置项
      </ElAlert>

      <div v-else-if="segmentItems.length" class="config-form-content">
        <div v-for="item in segmentItems" :key="item.key" class="config-card">
          <div class="config-card-header">
            <span class="config-card-title">{{ item.name || item.key }}</span>
          </div>
          <p v-if="item.description" class="config-description">{{ item.description }}</p>

          <!-- 模型选择 -->
          <div v-if="item.kind === 'llm'" class="model-selector">
            <ModelSelectorComponent
              :model_spec="agentStore.agentConfig[item.key] || ''"
              @select-model="(spec) => handleModelChange(item.key, spec)"
            />
          </div>

          <!-- 系统提示词 -->
          <div v-else-if="item.kind === 'prompt'" class="system-prompt-container">
            <div class="system-prompt-display" @click="openSystemPromptModal(item)">
              <div
                class="system-prompt-content"
                :class="{ 'is-placeholder': !agentStore.agentConfig[item.key] }"
              >
                {{ agentStore.agentConfig[item.key] || getPlaceholder(item) }}
              </div>
              <div class="edit-hint">点击查看并编辑</div>
            </div>
          </div>

          <!-- 布尔 -->
          <ElSwitch
            v-else-if="typeof agentStore.agentConfig[item.key] === 'boolean' || item.type === 'bool'"
            :model-value="!!agentStore.agentConfig[item.key]"
            @update:model-value="updateConfigValue(item.key, $event)"
          />

          <!-- 单选 -->
          <ElSelect
            v-else-if="
              getConfigOptions(item).length > 0 && (item.type === 'str' || item.type === 'select')
            "
            :model-value="agentStore.agentConfig[item.key]"
            class="w-full"
            @update:model-value="updateConfigValue(item.key, $event)"
          >
            <ElOption
              v-for="option in getConfigOptions(item)"
              :key="getOptionValue(option)"
              :label="getOptionLabel(option)"
              :value="getOptionValue(option)"
            />
          </ElSelect>

          <!-- 多选 / 资源列表 -->
          <div v-else-if="isListConfig(item)" class="list-config-container">
            <div class="multi-select-label">
              <span>
                已选择 {{ getSelectedCount(item.key, item) }} 项 | 共
                {{ getConfigOptions(item).length }} 项
              </span>
              <div class="label-actions">
                <ElButton
                  v-if="isToolsKind(item.kind)"
                  link
                  size="small"
                  class="clear-btn"
                  :disabled="getSelectedCount(item.key, item) === 0"
                  @click="clearSelection(item.key)"
                >
                  清空
                </ElButton>
                <template v-if="isToolsKind(item.kind)">
                  <ElButton link size="small" class="inline-action-btn" @click="refreshConfigOptions">
                    <ArtSvgIcon icon="ri:refresh-line" />
                    刷新
                  </ElButton>
                  <ElButton
                    link
                    size="small"
                    class="inline-action-btn"
                    @click="navigateToConfigPage(item.kind)"
                  >
                    <ArtSvgIcon icon="ri:settings-3-line" />
                    配置
                  </ElButton>
                </template>
                <ElButton
                  v-if="getConfigOptions(item).length > 5"
                  type="primary"
                  size="small"
                  class="selection-trigger-btn"
                  @click="openSelectionModal(item)"
                >
                  选择...
                </ElButton>
              </div>
            </div>

            <div v-if="getConfigOptions(item).length <= 5" class="multi-select-cards">
              <div class="options-grid">
                <div
                  v-for="option in getConfigOptions(item)"
                  :key="getOptionValue(option)"
                  class="option-card"
                  :class="{
                    selected: isOptionSelected(item.key, getOptionValue(option)),
                    unselected: !isOptionSelected(item.key, getOptionValue(option))
                  }"
                  @click="toggleOption(item.key, getOptionValue(option))"
                >
                  <div class="option-content">
                    <span class="option-text">{{ getOptionLabel(option) }}</span>
                    <div class="option-indicator">
                      <ArtSvgIcon
                        :icon="
                          isOptionSelected(item.key, getOptionValue(option))
                            ? 'ri:check-line'
                            : 'ri:add-line'
                        "
                      />
                    </div>
                  </div>
                </div>
              </div>
            </div>

            <div v-else-if="getSelectedCount(item.key, item) > 0" class="selection-preview">
              <ElTag
                v-for="val in ensureArray(item.key, item)"
                :key="val"
                closable
                class="selection-tag"
                @close="toggleOption(item.key, val)"
              >
                {{ getOptionLabelFromValue(item.key, val, item) }}
              </ElTag>
            </div>
          </div>

          <!-- 数字 -->
          <ElInputNumber
            v-else-if="['number', 'int', 'float'].includes(item.type)"
            :model-value="agentStore.agentConfig[item.key]"
            class="w-full"
            :placeholder="getPlaceholder(item)"
            @update:model-value="updateConfigValue(item.key, $event)"
          />

          <!-- 滑块 -->
          <ElSlider
            v-else-if="item.type === 'slider'"
            :model-value="agentStore.agentConfig[item.key] || 0"
            :min="item.min || 0"
            :max="item.max || 100"
            :step="item.step || 1"
            class="w-full"
            @update:model-value="updateConfigValue(item.key, $event)"
          />

          <!-- 默认文本 -->
          <ElInput
            v-else
            :model-value="agentStore.agentConfig[item.key]"
            :placeholder="getPlaceholder(item)"
            @update:model-value="updateConfigValue(item.key, $event)"
          />

          <div
            v-if="shouldShowKnowledgeSkillWarning(item.key)"
            class="knowledge-skill-warning"
            role="status"
          >
            <ArtSvgIcon icon="ri:alert-line" />
            <span>
              已启用知识库，但未选择 knowledge-base Skill。Agent
              可能无法调用知识库检索、打开文档等工具。
            </span>
          </div>
        </div>
      </div>

      <div v-else class="empty-config">暂无配置项</div>
    </div>

    <ElDialog
      v-model="selectionModalOpen"
      :title="`选择${currentConfigItem?.name || '项目'}`"
      width="760px"
      :close-on-click-modal="true"
      append-to-body
      class="agent-selection-dialog"
    >
      <div class="selection-modal-content">
        <div class="selection-search">
          <ElInput v-model="selectionSearchText" placeholder="搜索..." clearable>
            <template #prefix>
              <ArtSvgIcon icon="ri:search-line" />
            </template>
          </ElInput>
          <template v-if="isToolsKind(currentConfigItem?.kind)">
            <ElButton link size="small" class="inline-action-btn" @click="refreshConfigOptions">
              <ArtSvgIcon icon="ri:refresh-line" />
              刷新
            </ElButton>
            <ElButton
              link
              size="small"
              class="inline-action-btn"
              @click="navigateToConfigPage(currentConfigItem?.kind)"
            >
              <ArtSvgIcon icon="ri:settings-3-line" />
              配置
            </ElButton>
          </template>
        </div>

        <div class="selection-list">
          <div
            v-for="option in filteredOptions"
            :key="getOptionValue(option)"
            class="selection-item"
            :class="{ selected: tempSelectedValues.includes(getOptionValue(option)) }"
            @click="toggleModalSelection(getOptionValue(option))"
          >
            <div class="selection-item-content">
              <div class="selection-item-header">
                <span class="selection-item-name">{{ getOptionLabel(option) }}</span>
                <div class="selection-item-indicator">
                  <ArtSvgIcon
                    :icon="
                      tempSelectedValues.includes(getOptionValue(option))
                        ? 'ri:check-line'
                        : 'ri:add-line'
                    "
                  />
                </div>
              </div>
              <div v-if="getOptionDescription(option)" class="selection-item-description">
                {{ getOptionDescription(option) }}
              </div>
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <div class="selection-modal-footer">
          <span class="selected-count">已选择 {{ tempSelectedValues.length }} 项</span>
          <div class="modal-actions">
            <ElButton @click="closeSelectionModal">取消</ElButton>
            <ElButton type="primary" @click="confirmSelection">确认</ElButton>
          </div>
        </div>
      </template>
    </ElDialog>

    <ElDialog
      v-model="systemPromptModalOpen"
      :title="systemPromptModalTitle"
      width="620px"
      :close-on-click-modal="true"
      :show-close="false"
      append-to-body
      class="system-prompt-dialog extension-dialog"
      header-class="extension-dialog-header system-prompt-dialog-header"
      body-class="extension-dialog-body system-prompt-dialog-body"
      footer-class="extension-dialog-footer system-prompt-dialog-footer"
    >
      <ElInput
        v-model="systemPromptDraft"
        type="textarea"
        :rows="14"
        :placeholder="systemPromptModalPlaceholder"
      />
      <template #footer>
        <div class="system-prompt-modal-footer">
          <ElButton
            v-if="hasSystemPromptDefault"
            :disabled="isSystemPromptDefault"
            @click="restoreSystemPromptDefault"
          >
            恢复默认
          </ElButton>
          <div class="system-prompt-modal-actions">
            <ElButton @click="closeSystemPromptModal">取消</ElButton>
            <ElButton type="primary" @click="saveSystemPrompt">保存</ElButton>
          </div>
        </div>
      </template>
    </ElDialog>
  </div>
</template>

<script setup>
  import { ElMessage } from 'element-plus'
  import { useRouter } from 'vue-router'
  import ModelSelectorComponent from '@/components/chat/ModelSelectorComponent.vue'
  import { useAgentStore } from '@/store/modules/agent'
  import {
    getAgentConfigOptionDescription as getOptionDescription,
    getAgentConfigOptionLabel as getOptionLabel,
    getAgentConfigOptions as getConfigOptions,
    getAgentConfigOptionValue as getOptionValue,
    isDefaultAllAgentResourceKind
  } from '@/utils/agentConfigUtils'

  const props = defineProps({
    segment: {
      type: String,
      default: 'model'
    }
  })

  const agentStore = useAgentStore()
  const router = useRouter()

  const KNOWLEDGE_BASE_SKILL_SLUG = 'knowledge-base'

  const selectionModalOpen = ref(false)
  const currentConfigItem = ref(null)
  const tempSelectedValues = ref([])
  const selectionSearchText = ref('')
  const systemPromptModalOpen = ref(false)
  const currentSystemPromptItem = ref(null)
  const systemPromptDraft = ref('')

  const isEmptyConfig = computed(
    () => !agentStore.selectedAgentId || !agentStore.configurableItems.length
  )

  const segmentItems = computed(() => {
    const items = agentStore.configurableItems
    if (props.segment === 'model') {
      return items.filter((item) => item.kind === 'llm' || item.kind === 'prompt')
    }
    if (props.segment === 'tools') {
      return items.filter((item) => isDefaultAllAgentResourceKind(item.kind))
    }
    return items.filter(
      (item) =>
        item.kind !== 'llm' &&
        item.kind !== 'prompt' &&
        !isDefaultAllAgentResourceKind(item.kind)
    )
  })

  const systemPromptModalTitle = computed(
    () => currentSystemPromptItem.value?.name || currentSystemPromptItem.value?.key || 'System Prompt'
  )

  const systemPromptModalPlaceholder = computed(() =>
    currentSystemPromptItem.value ? getPlaceholder(currentSystemPromptItem.value) : '请输入系统提示词'
  )

  const hasSystemPromptDefault = computed(
    () =>
      !!currentSystemPromptItem.value &&
      Object.prototype.hasOwnProperty.call(currentSystemPromptItem.value, 'default')
  )

  const systemPromptDefaultValue = computed(() => {
    if (!hasSystemPromptDefault.value) return ''
    const defaultValue = currentSystemPromptItem.value.default
    return defaultValue == null ? '' : String(defaultValue)
  })

  const isSystemPromptDefault = computed(
    () => hasSystemPromptDefault.value && systemPromptDraft.value === systemPromptDefaultValue.value
  )

  const filteredOptions = computed(() => {
    if (!currentConfigItem.value) return []
    const options = getConfigOptions(currentConfigItem.value)
    const search = selectionSearchText.value.trim().toLowerCase()
    if (!search) return options
    return options.filter((opt) => {
      const label = String(getOptionLabel(opt)).toLowerCase()
      const desc = String(getOptionDescription(opt) || '').toLowerCase()
      return label.includes(search) || desc.includes(search)
    })
  })

  const isToolsKind = (kind) => isDefaultAllAgentResourceKind(kind)

  const isListConfig = (item) => {
    return (
      isDefaultAllAgentResourceKind(item?.kind) ||
      item?.type === 'list' ||
      item.key === 'skills' ||
      item.key === 'subagents'
    )
  }

  const getPlaceholder = (item) => {
    if (item?.default === undefined || item?.default === null) return ''
    return `（默认: ${item.default}）`
  }

  const updateConfigValue = (key, value) => {
    agentStore.updateAgentConfig({ [key]: value })
  }

  const handleModelChange = (key, spec) => {
    if (typeof spec !== 'string' || !spec) return
    updateConfigValue(key, spec)
  }

  const ensureArray = (key, item) => {
    const config = agentStore.agentConfig || {}
    const value = config[key]
    if (
      (value === null || value === undefined) &&
      isDefaultAllAgentResourceKind(item?.kind)
    ) {
      return getConfigOptions(item).map((option) => getOptionValue(option))
    }
    if (!Array.isArray(value)) return []
    const validValues = new Set(
      getConfigOptions(item).map((option) => String(getOptionValue(option)))
    )
    if (!validValues.size) return value
    return value.filter((entry) => validValues.has(String(entry)))
  }

  const isOptionSelected = (key, option) => {
    const item = agentStore.configurableItems.find((entry) => entry.key === key)
    const selected = ensureArray(key, item)
    const target = String(option)
    return selected.some((entry) => String(entry) === target)
  }

  const getSelectedCount = (key, item) => {
    const configItem = item || agentStore.configurableItems.find((entry) => entry.key === key)
    return ensureArray(key, configItem).length
  }

  const toggleOption = (key, option) => {
    const item = agentStore.configurableItems.find((entry) => entry.key === key)
    const currentOptions = [...ensureArray(key, item)]
    const target = String(option)
    const index = currentOptions.findIndex((entry) => String(entry) === target)
    if (index > -1) currentOptions.splice(index, 1)
    else currentOptions.push(option)
    updateConfigValue(key, currentOptions)
  }

  const clearSelection = (key) => updateConfigValue(key, [])

  const getOptionLabelFromValue = (key, val, item) => {
    const configItem = item || agentStore.configurableItems.find((entry) => entry.key === key)
    const option = getConfigOptions(configItem).find((opt) => getOptionValue(opt) === val)
    return option ? getOptionLabel(option) : val
  }

  const openSelectionModal = (item) => {
    currentConfigItem.value = item
    tempSelectedValues.value = [...ensureArray(item.key, item)]
    selectionSearchText.value = ''
    selectionModalOpen.value = true
  }

  const toggleModalSelection = (optionValue) => {
    const index = tempSelectedValues.value.indexOf(optionValue)
    if (index > -1) tempSelectedValues.value.splice(index, 1)
    else tempSelectedValues.value.push(optionValue)
  }

  const confirmSelection = () => {
    if (currentConfigItem.value) {
      updateConfigValue(currentConfigItem.value.key, [...tempSelectedValues.value])
    }
    closeSelectionModal()
  }

  const closeSelectionModal = () => {
    selectionModalOpen.value = false
    currentConfigItem.value = null
    tempSelectedValues.value = []
    selectionSearchText.value = ''
  }

  const openSystemPromptModal = (item) => {
    currentSystemPromptItem.value = item
    systemPromptDraft.value = agentStore.agentConfig[item.key] || ''
    systemPromptModalOpen.value = true
  }

  const closeSystemPromptModal = () => {
    systemPromptModalOpen.value = false
    currentSystemPromptItem.value = null
    systemPromptDraft.value = ''
  }

  const restoreSystemPromptDefault = () => {
    if (!hasSystemPromptDefault.value) return
    systemPromptDraft.value = systemPromptDefaultValue.value
  }

  const saveSystemPrompt = () => {
    if (!currentSystemPromptItem.value) return
    updateConfigValue(currentSystemPromptItem.value.key, systemPromptDraft.value)
    closeSystemPromptModal()
  }

  const refreshConfigOptions = async () => {
    if (!agentStore.selectedAgentId) return
    try {
      await agentStore.fetchAgentDetail(agentStore.selectedAgentId, true)
      ElMessage.success('配置选项已刷新')
    } catch (error) {
      console.error(error)
      ElMessage.error('刷新失败')
    }
  }

  const navigateToConfigPage = (kind) => {
    closeSelectionModal()
    setTimeout(() => {
      switch (kind) {
        case 'knowledges':
          router.push('/extensions/knowledge')
          break
        case 'tools':
          router.push('/extensions/tools')
          break
        case 'mcps':
          router.push('/extensions/mcp')
          break
        case 'skills':
          router.push('/extensions/skills')
          break
        case 'subagents':
          router.push('/agent/manage')
          break
      }
    }, 100)
  }

  const isResourceEnabled = (value) => {
    if (Array.isArray(value)) return value.length > 0
    return value === null || value === undefined
  }

  const isKnowledgeBaseSkillEnabled = computed(() => {
    const skills = agentStore.agentConfig?.skills
    if (Array.isArray(skills)) return skills.includes(KNOWLEDGE_BASE_SKILL_SLUG)
    return skills === null || skills === undefined
  })

  const shouldShowKnowledgeSkillWarning = (key) => {
    if (key !== 'knowledges') return false
    return (
      isResourceEnabled(agentStore.agentConfig?.knowledges) && !isKnowledgeBaseSkillEnabled.value
    )
  }
</script>

<style scoped>
  .agent-runtime-config-form {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .runtime-config-content {
    flex: 1;
    min-height: 0;
  }

  .config-alert {
    margin-bottom: 12px;
  }

  .config-form-content {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .config-card {
    padding: 12px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-fill-color-lighter);
  }

  .config-card-header {
    margin-bottom: 4px;
  }

  .config-card-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .config-description {
    margin: 0 0 10px;
    font-size: 12px;
    line-height: 1.45;
    color: var(--el-text-color-secondary);
  }

  .model-selector {
    display: block;
    width: 100%;
  }

  .model-selector :deep(.el-popover),
  .model-selector :deep(.el-popover__reference),
  .model-selector :deep(.el-popover__reference-wrapper) {
    display: block;
    width: 100%;
  }

  .model-selector :deep(.model-select) {
    display: flex;
    align-items: center;
    width: 100%;
    min-height: 36px;
    padding: 0 12px;
    border: 1px solid var(--el-border-color);
    border-radius: 8px;
    background: var(--el-bg-color);
    color: var(--el-text-color-primary);
    font-size: 13px;
    text-align: left;
  }

  .model-selector :deep(.model-select:hover) {
    border-color: var(--el-color-primary-light-5);
    background: var(--el-bg-color);
    color: var(--el-text-color-primary);
  }

  .model-selector :deep(.model-text) {
    max-width: none;
    flex: 1;
  }

  .system-prompt-container,
  .list-config-container {
    width: 100%;
  }

  .system-prompt-display {
    position: relative;
    min-height: 60px;
    padding: 10px 12px;
    border: 1px solid var(--el-border-color);
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s ease;
  }

  .system-prompt-display:hover {
    border-color: var(--el-color-primary);
    background: var(--el-fill-color-light);
  }

  .system-prompt-display:hover .edit-hint {
    opacity: 1;
  }

  .system-prompt-content {
    display: -webkit-box;
    overflow: hidden;
    color: var(--el-text-color-primary);
    font-size: 13px;
    line-height: 1.5;
    white-space: pre-line;
    word-break: break-word;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 4;
  }

  .system-prompt-content.is-placeholder {
    color: var(--el-text-color-placeholder);
    font-style: italic;
  }

  .edit-hint {
    position: absolute;
    top: -28px;
    right: 0;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--el-bg-color);
    color: var(--el-color-primary);
    font-size: 12px;
    opacity: 0;
    transition: opacity 0.2s ease;
  }

  .knowledge-skill-warning {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    margin-top: 8px;
    padding: 8px 10px;
    border: 1px solid var(--el-color-warning-light-5);
    border-radius: 8px;
    background: var(--el-color-warning-light-9);
    color: var(--el-color-warning-dark-2);
    font-size: 12px;
    line-height: 1.5;
  }

  .empty-config {
    padding: 40px 0;
    color: var(--el-text-color-secondary);
    text-align: center;
    font-size: 14px;
  }

  .multi-select-label {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 10px;
    color: var(--el-text-color-secondary);
    font-size: 12px;
  }

  .label-actions {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .options-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(min(180px, 100%), 1fr));
    gap: 8px;
  }

  .option-card {
    min-width: 0;
    padding: 8px 12px;
    border: 1px solid var(--el-border-color);
    border-radius: 8px;
    background: var(--el-bg-color);
    cursor: pointer;
    transition: all 0.2s ease;
  }

  .option-card:hover {
    border-color: var(--el-color-primary);
  }

  .option-card.selected {
    border-color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);
  }

  .option-card.selected .option-indicator,
  .option-card.selected .option-text {
    color: var(--el-color-primary);
  }

  .option-card.selected .option-text {
    font-weight: 500;
  }

  .option-content {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-width: 0;
  }

  .option-text {
    flex: 1;
    min-width: 0;
    color: var(--el-text-color-regular);
    font-size: 13px;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }

  .option-indicator {
    flex-shrink: 0;
    color: var(--el-text-color-secondary);
    font-size: 16px;
  }

  .selection-trigger-btn {
    height: 28px;
    padding: 0 12px;
    font-size: 12px;
  }

  .clear-btn,
  .inline-action-btn {
    padding: 0;
    height: auto;
    font-size: 12px;
    font-weight: 600;
    color: var(--el-color-primary);
  }

  .clear-btn.is-disabled,
  .clear-btn:disabled {
    color: var(--el-text-color-placeholder);
  }

  .selection-preview {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 2px;
  }

  .selection-tag {
    margin: 0;
  }

  .selection-search {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
  }

  .selection-list {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 10px;
    max-height: 52vh;
    overflow-y: auto;
  }

  .selection-item {
    padding: 12px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 8px;
    background: var(--el-bg-color);
    cursor: pointer;
    transition: all 0.2s ease;
  }

  .selection-item:hover {
    border-color: var(--el-border-color);
    background: var(--el-fill-color-light);
  }

  .selection-item.selected {
    border-color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);
  }

  .selection-item-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .selection-item-name {
    flex: 1;
    min-width: 0;
    font-size: 14px;
    font-weight: 500;
    color: var(--el-text-color-primary);
  }

  .selection-item-indicator {
    flex-shrink: 0;
    color: var(--el-text-color-secondary);
  }

  .selection-item.selected .selection-item-name,
  .selection-item.selected .selection-item-indicator {
    color: var(--el-color-primary);
  }

  .selection-item-description {
    margin-top: 6px;
    color: var(--el-text-color-secondary);
    font-size: 12px;
    line-height: 1.4;
  }

  .selection-modal-footer,
  .system-prompt-modal-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
  }

  .system-prompt-modal-actions,
  .modal-actions {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin-left: auto;
  }

  .selected-count {
    padding: 4px 10px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 8px;
    background: var(--el-fill-color-light);
    color: var(--el-text-color-regular);
    font-size: 13px;
  }
</style>

<style>
  .system-prompt-dialog.el-dialog {
    --el-dialog-padding-primary: 12px;
    padding: 0 12px 12px !important;
  }

  .system-prompt-dialog .system-prompt-dialog-header,
  .system-prompt-dialog.el-dialog .el-dialog__header {
    padding: 14px 0 6px !important;
    margin-right: 0 !important;
  }

  .system-prompt-dialog .system-prompt-dialog-body,
  .system-prompt-dialog.el-dialog .el-dialog__body {
    padding: 0 !important;
  }

  .system-prompt-dialog .system-prompt-dialog-footer,
  .system-prompt-dialog.el-dialog .el-dialog__footer {
    padding: 8px 0 0 !important;
  }

  .system-prompt-dialog .system-prompt-modal-footer .el-button + .el-button {
    margin-left: 4px;
  }
</style>
