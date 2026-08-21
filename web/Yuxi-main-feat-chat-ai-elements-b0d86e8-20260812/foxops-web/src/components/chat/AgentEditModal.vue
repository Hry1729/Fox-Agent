<template>
  <ElDialog
    v-model="visible"
    :width="isEditMode ? '680px' : '580px'"
    :close-on-click-modal="true"
    :destroy-on-close="true"
    :show-close="false"
    class="agent-edit-dialog extension-dialog"
    :class="{ 'agent-edit-dialog--create': !isEditMode }"
    header-class="extension-dialog-header agent-edit-dialog-header"
    body-class="extension-dialog-body"
    @close="handleClose"
  >
    <template #header>
      <div class="agent-modal-titlebar">
        <span class="agent-modal-title">{{ modalTitle }}</span>
        <div class="agent-modal-actions">
          <ElButton :disabled="saving" @click="handleClose">取消</ElButton>
          <ElButton type="primary" :loading="saving" @click="handleSave">
            {{ activeTab !== 'basic' && isEditMode ? '保存配置' : '保存' }}
          </ElButton>
        </div>
      </div>
    </template>

    <div
      class="agent-modal-content"
      :class="{ 'without-sidebar': !isEditMode, 'create-mode': !isEditMode }"
    >
      <!-- 侧边栏（仅编辑模式） -->
      <aside v-if="isEditMode" class="agent-modal-sidebar" aria-label="智能体配置分组">
        <button
          v-for="tab in tabs"
          :key="tab.key"
          type="button"
          class="nav-item"
          :class="{ active: activeTab === tab.key }"
          @click="activeTab = tab.key"
        >
          <span class="nav-item-main">
            <ArtSvgIcon :icon="tab.icon" class="nav-item-icon" />
            <span>{{ tab.label }}</span>
          </span>
          <span v-if="tab.key !== 'basic' && agentStore.hasConfigChanges" class="dirty-dot"></span>
        </button>
      </aside>

      <!-- 主区域 -->
      <div class="agent-modal-main">
        <!-- Tab 1: 基本信息 -->
        <section v-show="activeTab === 'basic'" class="agent-modal-section basic-pane">
          <div class="agent-basic-body">
            <div class="agent-identity-row" aria-label="智能体图标、名称与后端">
              <div class="agent-profile-main">
                <ElUpload
                  :show-file-list="false"
                  :before-upload="beforeAgentIconUpload"
                  :disabled="iconUploading"
                  accept="image/*"
                >
                  <div
                    class="agent-icon-upload"
                    :class="{
                      uploading: iconUploading,
                      'is-empty': !form.icon && !isEditMode
                    }"
                  >
                    <FallbackAvatar
                      v-if="form.icon || isEditMode"
                      :src="form.icon"
                      :default-src="agentPreviewDefaultIcon"
                      :name="agentPreviewName"
                      :seed="editingAgentId || form.slug || form.name"
                      kind="agent"
                      :size="56"
                      shape="rounded"
                      :alt="`${form.name || '智能体'}图标`"
                      class="agent-icon-preview-avatar"
                    />
                    <div class="agent-icon-mask">
                      <ArtSvgIcon
                        :icon="iconUploading ? 'ri:loader-4-line' : 'ri:upload-2-line'"
                        :class="{ spinning: iconUploading }"
                      />
                      <span>{{ form.icon ? '更换图标' : '上传图标' }}</span>
                    </div>
                  </div>
                </ElUpload>
                <div class="agent-icon-preview-text">
                  <input
                    ref="agentNameInputRef"
                    v-model="form.name"
                    class="agent-inline-name-input"
                    type="text"
                    placeholder="点击输入智能体名称"
                    aria-label="智能体名称"
                  />
                  <input
                    v-if="!isEditMode"
                    v-model="form.slug"
                    class="agent-inline-slug-input"
                    type="text"
                    placeholder="标识可选，留空自动生成"
                    aria-label="智能体标识"
                  />
                  <span v-else class="agent-inline-slug">{{ form.slug || editingAgentId }}</span>
                </div>
              </div>

              <div
                class="agent-backend-summary"
                :class="{ editable: !isEditMode }"
                aria-label="智能体后端"
              >
                <span class="agent-backend-icon">
                  <ArtSvgIcon :icon="selectedBackendIcon" />
                </span>
                <div class="agent-backend-text">
                  <span class="agent-backend-label">智能体后端</span>
                  <ElSelect
                    v-if="!isEditMode"
                    v-model="form.backend_id"
                    class="agent-backend-select"
                    :teleported="false"
                  >
                    <ElOption
                      v-for="b in agentStore.agentBackends"
                      :key="b.value"
                      :label="b.label"
                      :value="b.value"
                    />
                  </ElSelect>
                  <span v-else class="agent-backend-name">{{ selectedBackendLabel }}</span>
                </div>
              </div>
            </div>

            <label class="agent-description-row">
              <span class="field-label">描述</span>
              <ElInput
                v-model="form.description"
                type="textarea"
                :rows="3"
                placeholder="可选"
              />
            </label>

            <div v-if="canEditShareConfig" class="agent-basic-divider"></div>

            <div v-if="canEditShareConfig" class="agent-share-row">
              <div class="section-heading">共享权限</div>
              <ShareScopeForm
                v-model="shareConfig"
                variant="cards"
                :auto-select-user-dept="true"
                :allowed-access-levels="shareAllowedLevels"
              />
            </div>

            <p v-else-if="isEditMode && isEditingBuiltin" class="share-readonly-hint">
              内置智能体固定为全局生效范围。
            </p>
          </div>
        </section>

        <!-- Tab 2: 模型配置 -->
        <section v-if="isEditMode" v-show="activeTab === 'model'" class="agent-modal-section runtime-section">
          <div v-if="agentStore.hasConfigChanges" class="dirty-notice">
            <ArtSvgIcon icon="ri:error-warning-line" class="text-sm" />
            <span>有未保存的配置修改</span>
            <span class="link" @click="agentStore.resetAgentConfig()">恢复</span>
          </div>
          <AgentRuntimeConfigForm segment="model" />
        </section>

        <!-- Tab 3: 工具配置 -->
        <section v-if="isEditMode" v-show="activeTab === 'tools'" class="agent-modal-section runtime-section">
          <div v-if="agentStore.hasConfigChanges" class="dirty-notice">
            <ArtSvgIcon icon="ri:error-warning-line" class="text-sm" />
            <span>有未保存的配置修改</span>
            <span class="link" @click="agentStore.resetAgentConfig()">恢复</span>
          </div>
          <AgentRuntimeConfigForm segment="tools" />
        </section>

        <!-- Tab 4: 其他配置 -->
        <section v-if="isEditMode" v-show="activeTab === 'other'" class="agent-modal-section runtime-section">
          <div v-if="agentStore.hasConfigChanges" class="dirty-notice">
            <ArtSvgIcon icon="ri:error-warning-line" class="text-sm" />
            <span>有未保存的配置修改</span>
            <span class="link" @click="agentStore.resetAgentConfig()">恢复</span>
          </div>
          <AgentRuntimeConfigForm segment="other" />
        </section>
      </div>
    </div>
  </ElDialog>
</template>

<script setup>
  import { ElMessage } from 'element-plus'
  import { multimodalApi } from '@/api/agent'
  import AgentRuntimeConfigForm from '@/components/agent-management/AgentRuntimeConfigForm.vue'
  import FallbackAvatar from '@/components/common/FallbackAvatar.vue'
  import ShareScopeForm from '@/components/extensions/common/ShareScopeForm.vue'
  import { isBuiltinAgent, useAgentStore } from '@/store/modules/agent'
  import { useUserStore } from '@/store/modules/user'
  import { generatePixelAvatar } from '@/utils/pixelAvatar'

  const MAX_IMAGE_UPLOAD_SIZE_BYTES = 2 * 1024 * 1024

  const emit = defineEmits(['saved'])

  const agentStore = useAgentStore()
  const userStore = useUserStore()

  const visible = ref(false)
  const isEditMode = ref(false)
  const activeTab = ref('basic')
  const saving = ref(false)
  const iconUploading = ref(false)
  const editingAgentId = ref(null)
  const agentNameInputRef = ref(null)

  const form = reactive({
    name: '',
    slug: '',
    icon: '',
    backend_id: 'ChatbotAgent',
    description: ''
  })

  const shareConfig = ref({
    access_level: 'user',
    department_ids: [],
    user_uids: []
  })

  const modalTitle = computed(() => (isEditMode.value ? '编辑智能体' : '新增智能体'))
  const isEditingBuiltin = computed(() => isBuiltinAgent({ id: editingAgentId.value }))
  const canEditShareConfig = computed(() => !isEditingBuiltin.value)
  const shareAllowedLevels = computed(() => {
    if (isEditingBuiltin.value) return ['global']
    return userStore.isAdmin ? ['global', 'department', 'user'] : ['user']
  })
  const agentPreviewDefaultIcon = computed(() =>
    editingAgentId.value ? generatePixelAvatar(editingAgentId.value) : ''
  )
  const agentPreviewName = computed(() => form.name || editingAgentId.value || '智能体')
  const selectedBackendOption = computed(() =>
    agentStore.agentBackends.find((backend) => backend.value === form.backend_id)
  )
  const selectedBackendLabel = computed(
    () => selectedBackendOption.value?.label || form.backend_id || '未选择'
  )
  const selectedBackendIcon = computed(() => {
    const backendText = `${form.backend_id} ${selectedBackendLabel.value}`.toLowerCase()
    return backendText.includes('deep') || backendText.includes('search')
      ? 'ri:search-line'
      : 'ri:robot-2-line'
  })

  const getInitialShareConfig = () => ({
    access_level: userStore.isAdmin ? 'global' : 'user',
    department_ids: [],
    user_uids: userStore.info?.uid ? [String(userStore.info.uid)] : []
  })

  const normalizeShareConfigForPayload = () => {
    if (isEditingBuiltin.value) {
      return { access_level: 'global', department_ids: [], user_uids: [] }
    }
    const config = shareConfig.value || getInitialShareConfig()
    const accessLevel = userStore.isAdmin ? config.access_level : 'user'
    return {
      access_level: accessLevel,
      department_ids: accessLevel === 'department' ? config.department_ids || [] : [],
      user_uids: accessLevel === 'user' ? config.user_uids || [] : []
    }
  }

  const tabs = [
    { key: 'basic', label: '基本信息', icon: 'ri:information-line' },
    { key: 'model', label: '模型配置', icon: 'ri:equalizer-line' },
    { key: 'tools', label: '工具配置', icon: 'ri:tools-line' },
    { key: 'other', label: '其他配置', icon: 'ri:settings-3-line' }
  ]

  const focusAgentNameInput = async () => {
    await nextTick()
    agentNameInputRef.value?.focus?.()
  }

  const beforeAgentIconUpload = (file) => {
    if (!file.type.startsWith('image/')) {
      ElMessage.error('只能上传图片文件')
      return false
    }
    if (file.size > MAX_IMAGE_UPLOAD_SIZE_BYTES) {
      ElMessage.error('图片大小不能超过 2MB')
      return false
    }
    uploadAgentIcon(file)
    return false
  }

  const uploadAgentIcon = async (file) => {
    iconUploading.value = true
    try {
      const data = await multimodalApi.uploadImage(file)
      form.icon = data?.image_url || data?.url || ''
      ElMessage.success('图标上传成功')
    } catch (error) {
      ElMessage.error(error?.message || '图标上传失败')
    } finally {
      iconUploading.value = false
    }
  }

  const openCreate = async () => {
    isEditMode.value = false
    editingAgentId.value = null
    activeTab.value = 'basic'
    Object.assign(form, { name: '', slug: '', icon: '', backend_id: 'ChatbotAgent', description: '' })
    shareConfig.value = getInitialShareConfig()
    await agentStore.fetchAgentBackends()
    visible.value = true
    focusAgentNameInput()
  }

  const openEdit = async (agentOrId) => {
    const agentId = typeof agentOrId === 'string' ? agentOrId : agentOrId?.id
    if (!agentId) return
    isEditMode.value = true
    editingAgentId.value = agentId
    activeTab.value = 'basic'
    visible.value = true

    try {
      const agent = await agentStore.fetchAgentDetail(agentId, true)
      Object.assign(form, {
        name: agent.name || '',
        slug: agent.slug || agent.id,
        icon: agent.icon || '',
        backend_id: agent.backend_id || '',
        description: agent.description || ''
      })
      shareConfig.value = isBuiltinAgent(agent)
        ? { access_level: 'global', department_ids: [], user_uids: [] }
        : agent.share_config || getInitialShareConfig()
      await agentStore.selectAgent(agentId)
    } catch (e) {
      ElMessage.error('加载智能体详情失败')
      console.error(e)
    }
  }

  const close = () => {
    visible.value = false
  }

  const handleClose = () => {
    if (agentStore.hasConfigChanges) {
      agentStore.resetAgentConfig()
    }
    visible.value = false
  }

  const handleSave = async () => {
    if (!form.name?.trim()) {
      activeTab.value = 'basic'
      ElMessage.warning('请输入智能体名称')
      focusAgentNameInput()
      return
    }

    saving.value = true
    try {
      if (isEditMode.value) {
        await agentStore.updateAgentProfile(editingAgentId.value, {
          name: form.name,
          description: form.description,
          icon: form.icon,
          share_config: normalizeShareConfigForPayload()
        })
        if (agentStore.hasConfigChanges) {
          await agentStore.saveAgentConfig(editingAgentId.value)
        }
        emit('saved', { mode: 'edit', agentId: editingAgentId.value })
      } else {
        const agent = await agentStore.createAgent({
          name: form.name,
          slug: form.slug || undefined,
          icon: form.icon,
          description: form.description,
          backend_id: form.backend_id,
          share_config: normalizeShareConfigForPayload()
        })
        emit('saved', { mode: 'create', agent })
      }
      ElMessage.success('保存成功')
      visible.value = false
    } catch (e) {
      ElMessage.error(e?.message || '保存失败')
      console.error(e)
    } finally {
      saving.value = false
    }
  }

  defineExpose({ openCreate, openEdit, close })
</script>

<style scoped>
  .agent-modal-titlebar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    width: 100%;
  }

  .agent-modal-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .agent-modal-actions {
    display: inline-flex;
    align-items: center;
    gap: 0;
  }

  .agent-modal-actions :deep(.el-button) {
    margin: 0;
  }

  .agent-modal-actions :deep(.el-button + .el-button) {
    margin-left: 4px;
  }

  .agent-modal-content {
    display: grid;
    grid-template-columns: 132px minmax(0, 1fr);
    height: min(72vh, 560px);
    min-height: 480px;
    overflow: hidden;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 14px;
    background: var(--el-bg-color);
  }

  .agent-modal-content.without-sidebar {
    grid-template-columns: minmax(0, 1fr);
    height: auto;
    min-height: 0;
  }

  .agent-modal-content.create-mode {
    box-shadow: 0 8px 24px rgb(0 0 0 / 6%);
  }

  .agent-modal-sidebar {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-height: 0;
    padding: 12px 4px 12px 8px;
    overflow-y: auto;
    border-right: 1px solid var(--el-border-color-lighter);
    background: linear-gradient(
      180deg,
      var(--el-fill-color-light) 0%,
      var(--el-color-primary-light-9) 100%
    );
  }

  .nav-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    min-height: 38px;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: 10px;
    background: transparent;
    color: var(--el-text-color-regular);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.16s ease;
    text-align: left;
  }

  .nav-item-main {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .nav-item-icon {
    font-size: 16px;
    flex-shrink: 0;
  }

  .nav-item:hover {
    background: var(--el-bg-color);
    color: var(--el-text-color-primary);
  }

  .nav-item.active {
    border-color: var(--el-color-primary-light-7);
    background: var(--el-bg-color);
    color: var(--el-color-primary);
    font-weight: 600;
  }

  .dirty-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--el-color-warning);
  }

  .agent-modal-main {
    min-width: 0;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
    padding: 16px 4px 18px 18px;
    scrollbar-gutter: stable;
  }

  .agent-modal-section {
    min-height: 0;
  }

  .runtime-section {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .runtime-section :deep(.agent-runtime-config-form) {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .runtime-section :deep(.runtime-config-content) {
    flex: 1;
    min-height: 0;
    padding: 0;
    overflow: visible;
  }

  .basic-pane {
    min-height: auto;
  }

  .agent-basic-body {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .agent-identity-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    min-width: 0;
    gap: 12px;
  }

  .agent-profile-main {
    display: inline-flex;
    align-items: center;
    min-width: 0;
    flex: 1;
    gap: 10px;
  }

  .agent-icon-upload {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 56px;
    height: 56px;
    overflow: hidden;
    border: 1px solid var(--el-border-color);
    border-radius: 12px;
    background: var(--el-fill-color-lighter);
    cursor: pointer;
    transition:
      border-color 0.16s ease,
      box-shadow 0.16s ease;
  }

  .agent-icon-upload :deep(.fallback-avatar) {
    width: 100%;
    height: 100%;
  }

  .agent-icon-upload:hover,
  .agent-icon-upload:focus-within,
  .agent-icon-upload.uploading {
    border-color: var(--el-color-primary-light-5);
    box-shadow: 0 0 0 3px var(--el-color-primary-light-9);
  }

  .agent-icon-upload:hover .agent-icon-mask,
  .agent-icon-upload:focus-within .agent-icon-mask,
  .agent-icon-upload.uploading .agent-icon-mask,
  .agent-icon-upload.is-empty .agent-icon-mask {
    opacity: 1;
  }

  .agent-icon-upload.is-empty {
    border-style: dashed;
    background: var(--el-bg-color);
  }

  .agent-icon-mask {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    background: color-mix(in srgb, var(--el-text-color-primary) 62%, transparent);
    color: #fff;
    font-size: 11px;
    font-weight: 600;
    opacity: 0;
    transition: opacity 0.16s ease;
  }

  .agent-icon-upload.is-empty .agent-icon-mask {
    background: transparent;
    color: var(--el-text-color-secondary);
  }

  .agent-icon-mask .spinning {
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .agent-icon-preview-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    gap: 4px;
    line-height: 1.25;
  }

  .agent-inline-name-input {
    width: 180px;
    max-width: 100%;
    padding: 1px 4px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: var(--el-text-color-primary);
    caret-color: var(--el-color-primary);
    font-size: 14px;
    font-weight: 600;
    line-height: 1.35;
    transition:
      border-color 0.16s ease,
      background 0.16s ease,
      box-shadow 0.16s ease;
  }

  .agent-inline-name-input::placeholder {
    color: var(--el-text-color-placeholder);
  }

  .agent-inline-name-input:hover {
    border-color: var(--el-border-color);
    background: var(--el-bg-color);
  }

  .agent-inline-name-input:focus {
    border-color: var(--el-color-primary-light-5);
    background: var(--el-bg-color);
    box-shadow: 0 0 0 3px var(--el-color-primary-light-9);
    outline: none;
  }

  .agent-inline-slug,
  .agent-inline-slug-input {
    width: 180px;
    max-width: 100%;
    padding: 1px 4px;
    overflow: hidden;
    color: var(--el-text-color-secondary);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .agent-inline-slug-input {
    border: 1px solid transparent;
    border-radius: 2px;
    background: transparent;
  }

  .agent-inline-slug-input::placeholder {
    color: var(--el-text-color-placeholder);
  }

  .agent-inline-slug-input:hover,
  .agent-inline-slug-input:focus {
    border-color: var(--el-border-color);
    background: var(--el-bg-color);
    outline: none;
  }

  .agent-backend-summary {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
    gap: 8px;
    width: 156px;
    min-height: 50px;
    margin-left: auto;
    padding: 12px 8px 5px 10px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 12px;
    background: var(--el-fill-color-lighter);
    color: var(--el-text-color-regular);
  }

  .agent-backend-summary.editable {
    padding-right: 6px;
  }

  .agent-backend-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    background: var(--el-fill-color);
    font-size: 15px;
    color: var(--el-text-color-regular);
  }

  .agent-backend-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: flex-start;
    min-width: 0;
    gap: 2px;
    line-height: 1.2;
  }

  .agent-backend-label {
    color: var(--el-text-color-secondary);
    font-size: 11px;
    white-space: nowrap;
  }

  .agent-backend-name {
    max-width: 100%;
    overflow: hidden;
    color: var(--el-text-color-primary);
    font-size: 13px;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .agent-backend-select {
    width: 100%;
    margin: 0;
  }

  .agent-backend-select :deep(.el-select__wrapper) {
    min-height: 22px;
    padding: 0 16px 0 0;
    background: transparent;
    box-shadow: none !important;
  }

  .agent-backend-select :deep(.el-select__selection) {
    margin-left: 0;
  }

  .agent-backend-select :deep(.el-select__selected-item),
  .agent-backend-select :deep(.el-select__placeholder) {
    color: var(--el-text-color-primary);
    font-size: 13px;
    font-weight: 600;
  }

  .agent-backend-select :deep(.el-select__suffix) {
    right: 0;
  }

  .agent-backend-select :deep(.el-select__caret) {
    color: var(--el-text-color-secondary);
  }

  .agent-description-row {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .field-label {
    color: var(--el-text-color-regular);
    font-size: 12px;
    font-weight: 500;
  }

  .agent-basic-divider {
    height: 1px;
    background: var(--el-border-color-lighter);
  }

  .agent-share-row {
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: 100%;
  }

  .agent-share-row :deep(.share-scope-form.is-cards) {
    width: 100%;
  }

  .agent-share-row :deep(.share-mode-cards) {
    width: 100%;
    gap: 6px;
  }

  .agent-share-row :deep(.share-mode-card) {
    min-height: 68px;
    padding: 10px 8px;
    gap: 8px;
  }

  .agent-share-row :deep(.card-header) {
    gap: 6px;
  }

  .agent-share-row :deep(.card-title) {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    font-size: 13px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .agent-share-row :deep(.card-icon-wrapper) {
    width: 32px;
    height: 32px;
    border-radius: 8px;
  }

  .agent-share-row :deep(.card-icon) {
    font-size: 17px;
  }

  .agent-share-row :deep(.card-description) {
    font-size: 11px;
    line-height: 1.4;
  }

  .agent-share-row :deep(.select-action) {
    min-width: 36px;
    height: 22px;
    padding: 0 4px;
  }

  .section-heading {
    font-size: 14px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .share-readonly-hint {
    margin: 0;
    color: var(--el-text-color-secondary);
    font-size: 13px;
    line-height: 1.5;
  }

  .dirty-notice {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px;
    margin-bottom: 12px;
    border-radius: 6px;
    background: var(--el-color-warning-light-9);
    color: var(--el-color-warning-dark-2);
    font-size: 13px;
  }

  .dirty-notice .link {
    color: var(--el-color-primary);
    cursor: pointer;
    margin-left: auto;
  }
</style>

<style>
  .agent-edit-dialog.el-dialog {
    width: 680px !important;
    max-width: 680px;
  }

  .agent-edit-dialog.agent-edit-dialog--create.el-dialog {
    width: 580px !important;
    max-width: 580px;
  }
  .agent-edit-dialog.el-dialog .el-dialog__header {
    padding-bottom: 8px !important;
    margin-right: 0 !important;
  }

  .agent-edit-dialog.el-dialog .el-dialog__body {
    padding-top: 4px !important;
  }
</style>
