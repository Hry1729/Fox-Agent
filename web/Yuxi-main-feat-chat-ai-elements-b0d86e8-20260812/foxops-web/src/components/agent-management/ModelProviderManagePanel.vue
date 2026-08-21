<template>
  <div class="model-provider-manage-panel">
    <ExtensionToolbar
      v-model:search="searchQuery"
      search-placeholder="搜索供应商..."
      :loading="loading"
      @refresh="loadProviders"
    >
      <template #actions>
        <ElButton type="primary" @click="openCreateProviderModal">
          <ArtSvgIcon icon="ri:add-line" />
          新增供应商
        </ElButton>
      </template>
    </ExtensionToolbar>

    <ExtensionEmptyState
      v-if="!loading && !filteredProviders.length"
      :description="searchQuery ? '无匹配供应商' : '暂无模型供应商，点击上方按钮添加'"
    />

    <ExtensionCardGrid v-else class="provider-card-grid">
      <ExtensionInfoCard
        v-for="provider in filteredProviders"
        :key="provider.provider_id"
        class="provider-info-card"
        :title="provider.display_name"
        :subtitle="provider.provider_id"
        description=""
        accent="blue"
        :brand-icon-url="getProviderIcon(provider)"
        :tags="getProviderTags(provider)"
        @click="openEditProviderModal(provider)"
      >
        <template #description>
          <p class="provider-desc-line provider-desc-url" :title="provider.base_url || '-'">
            Base URL: {{ provider.base_url || '-' }}
          </p>
        </template>
        <template #toolbar>
          <div class="provider-card-actions">
            <ElButton text class="view-models-btn" @click.stop="openModelsModal(provider)">
              <ArtSvgIcon icon="ri:stack-line" class="view-models-icon" />
              管理模型
              <span v-if="provider.enabled_models?.length" class="enabled-count">
                （已启用 {{ provider.enabled_models.length }} 个）
              </span>
            </ElButton>
            <ElSwitch
              :model-value="!!provider.is_enabled"
              :loading="togglingProviderId === provider.provider_id"
              @change="(checked) => toggleProviderEnabled(provider, !!checked)"
              @click.stop
            />
          </div>
        </template>
      </ExtensionInfoCard>
    </ExtensionCardGrid>

    <!-- 供应商编辑弹窗 -->
    <ElDialog
      v-model="showProviderModal"
      width="560px"
      destroy-on-close
      :close-on-click-modal="true"
      :show-close="false"
      class="extension-dialog model-provider-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
      footer-class="extension-dialog-footer"
    >
      <template #header>
        <ExtensionDialogHeader
          :title="editingProviderId ? '编辑供应商' : '新增供应商'"
          @close="showProviderModal = false"
        />
      </template>

      <div class="modal-form">
        <div class="form-row">
          <label class="form-label">
            <span>Provider ID</span>
            <ElInput
              v-model="providerForm.provider_id"
              :disabled="!!editingProviderId"
              placeholder="my-provider"
            />
          </label>
          <label class="form-label">
            <span>展示名称</span>
            <ElInput v-model="providerForm.display_name" placeholder="My Provider" />
          </label>
        </div>

        <div class="form-row">
          <label class="form-label">
            <span>Base URL</span>
            <ElInput v-model="providerForm.base_url" placeholder="https://api.example.com/v1" />
          </label>
          <label class="form-label">
            <span>Provider Type</span>
            <ElSelect v-model="providerForm.provider_type" style="width: 100%">
              <ElOption value="openai" label="openai" />
            </ElSelect>
          </label>
        </div>

        <div class="form-row">
          <label class="form-label">
            <span>API Key Env</span>
            <ElInput v-model="providerForm.api_key_env" placeholder="环境变量名" />
          </label>
          <label class="form-label">
            <span>API Key</span>
            <ElInput
              v-model="providerForm.api_key"
              type="password"
              show-password
              placeholder="直接配置 API Key"
            />
          </label>
        </div>

        <div class="form-row">
          <label class="form-label full-width">
            <span>Models Endpoint</span>
            <ElInput v-model="providerForm.models_endpoint" placeholder="/models" />
          </label>
        </div>

        <template v-if="providerForm.capabilities.includes('embedding')">
          <div class="form-row">
            <label class="form-label">
              <span>Embedding Base URL</span>
              <ElInput
                v-model="providerForm.embedding_base_url"
                placeholder="https://api.example.com/v1/embeddings"
              />
            </label>
            <label class="form-label">
              <span>Embedding Endpoint</span>
              <ElInput
                v-model="providerForm.embedding_models_endpoint"
                placeholder="/embeddings/models"
              />
            </label>
          </div>
        </template>

        <template v-if="providerForm.capabilities.includes('rerank')">
          <div class="form-row">
            <label class="form-label">
              <span>Rerank Base URL</span>
              <ElInput
                v-model="providerForm.rerank_base_url"
                placeholder="https://api.example.com/v1/rerank"
              />
            </label>
            <label class="form-label">
              <span>Rerank Endpoint</span>
              <ElInput
                v-model="providerForm.rerank_models_endpoint"
                placeholder="按供应商文档填写，留空则不自动加载"
              />
            </label>
          </div>
        </template>

        <label class="form-label full-width">
          <span>能力</span>
          <ElSelect v-model="providerForm.capabilities" multiple style="width: 100%">
            <ElOption value="chat" label="chat" />
            <ElOption value="embedding" label="embedding" />
            <ElOption value="rerank" label="rerank" />
          </ElSelect>
        </label>

        <div class="form-switch">
          <span>状态</span>
          <ElSwitch
            v-model="providerForm.is_enabled"
            active-text="启用"
            inactive-text="停用"
          />
        </div>

        <ElCollapse class="advanced-collapse">
          <ElCollapseItem title="高级配置" name="advanced">
            <label class="form-label full-width">
              <span>请求头 JSON</span>
              <ElInput
                v-model="providerForm.headers_text"
                type="textarea"
                :rows="4"
                placeholder="{}"
              />
            </label>
            <label class="form-label full-width">
              <span>扩展配置 JSON</span>
              <ElInput
                v-model="providerForm.extra_text"
                type="textarea"
                :rows="4"
                placeholder="{}"
              />
            </label>
          </ElCollapseItem>
        </ElCollapse>
      </div>

      <template #footer>
        <div class="provider-modal-footer">
          <ElButton v-if="editingProviderId" type="danger" @click="deleteProviderFromEdit">
            <ArtSvgIcon icon="ri:delete-bin-line" />
            删除供应商
          </ElButton>
          <span v-else />
          <div class="provider-modal-footer-actions">
            <ElButton @click="showProviderModal = false">取消</ElButton>
            <ElButton
              type="primary"
              :loading="saving"
              @click="editingProviderId ? saveProvider() : createProvider()"
            >
              确认
            </ElButton>
          </div>
        </div>
      </template>
    </ElDialog>

    <!-- 模型管理弹窗 -->
    <ElDialog
      v-model="showModelsModal"
      width="800px"
      destroy-on-close
      :show-close="false"
      class="extension-dialog model-provider-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
    >
      <template #header>
        <ExtensionDialogHeader
          :title="
            currentProviderForModels
              ? `${currentProviderForModels.display_name} - 模型配置`
              : '模型配置'
          "
          @close="showModelsModal = false"
        />
      </template>

      <div v-if="currentProviderForModels" class="models-modal-content">
        <div class="models-section">
          <div class="enabled-header">
            <h4 class="models-section-title">
              已启用模型 ({{ currentProviderForModels.enabled_models?.length || 0 }})
            </h4>
            <div class="actions">
              <ElButton
                size="small"
                type="primary"
                :loading="remoteLoading"
                @click="fetchRemoteModels(currentProviderForModels.provider_id)"
              >
                获取远程模型
              </ElButton>
              <ElButton size="small" @click="openCreateModal(currentProviderForModels)">
                <ArtSvgIcon icon="ri:add-line" />
                手动添加
              </ElButton>
            </div>
          </div>

          <div v-if="currentProviderForModels.enabled_models?.length" class="models-table">
            <div class="table-head">
              <span class="col-name">模型</span>
              <span class="col-type">类型</span>
              <span class="col-context">上下文</span>
              <span class="col-dim">维度</span>
              <span class="col-ops">操作</span>
            </div>
            <div
              v-for="model in currentProviderForModels.enabled_models"
              :key="model.id"
              class="table-row"
              :class="{ stale: isModelStale(model, currentProviderForModels.provider_id) }"
            >
              <div class="model-info">
                <span class="model-name">{{ getModelDisplayName(model) }}</span>
                <span class="model-id">{{ getModelId(model) }}</span>
              </div>
              <span class="col-type">
                <span class="type-tag" :class="model.type">{{ model.type }}</span>
                <span
                  v-if="model.source === 'manual'"
                  class="type-tag manual"
                  title="管理员手动添加"
                >
                  <ArtSvgIcon icon="ri:stack-line" />
                </span>
              </span>
              <span class="col-context">{{ formatContextLength(model.context_length) }}</span>
              <span class="col-dim">
                <span
                  v-if="model.type === 'embedding' && !model.dimension"
                  class="dim-warning"
                  title="缺少维度配置"
                >
                  ⚠
                </span>
                <span v-else>{{ model.dimension || '-' }}</span>
              </span>
              <span class="col-ops">
                <ElButton
                  size="small"
                  class="model-test-button"
                  :class="{
                    'is-testing': isModelTesting(currentProviderForModels.provider_id, model.id)
                  }"
                  :title="getModelTestTitle(currentProviderForModels.provider_id, model)"
                  @click="testModelConnection(currentProviderForModels.provider_id, model)"
                >
                  <ArtSvgIcon
                    v-if="isModelTesting(currentProviderForModels.provider_id, model.id)"
                    icon="ri:loader-4-line"
                    class="spinning"
                  />
                  <ArtSvgIcon v-else icon="ri:flashlight-line" />
                </ElButton>
                <ElButton size="small" @click="openModelConfigModal(model)">
                  <ArtSvgIcon icon="ri:settings-3-line" />
                </ElButton>
                <ElButton
                  size="small"
                  type="danger"
                  @click="removeModel(currentProviderForModels.provider_id, model.id)"
                >
                  <ArtSvgIcon icon="ri:delete-bin-line" />
                </ElButton>
              </span>
            </div>
          </div>
          <ElEmpty v-else description="暂无已启用模型" />
        </div>

        <div class="models-section">
          <div class="remote-header">
            <h4 class="models-section-title">远端候选模型 ({{ filteredRemoteModels.length }})</h4>
            <ElInput
              v-if="remoteModelsMap[currentProviderForModels.provider_id]?.length"
              v-model="remoteModelSearch[currentProviderForModels.provider_id]"
              class="remote-search-input"
              placeholder="搜索模型..."
              clearable
            >
              <template #prefix>
                <ArtSvgIcon icon="ri:search-line" />
              </template>
            </ElInput>
            <ElSegmented
              v-if="remoteModelsMap[currentProviderForModels.provider_id]?.length"
              v-model="remoteModelTypeFilter[currentProviderForModels.provider_id]"
              :options="remoteModelTypeOptions"
              class="remote-type-filter"
            />
          </div>

          <div
            v-if="remoteModelsMap[currentProviderForModels.provider_id]?.length"
            class="remote-list"
          >
            <div
              v-for="remoteModel in filteredRemoteModels"
              :key="remoteModel.id"
              class="remote-row"
            >
              <span class="remote-name">{{ getModelDisplayName(remoteModel) }}</span>
              <div class="remote-tags">
                <span class="type-tag" :class="remoteModel.type || 'chat'">
                  {{ remoteModel.type || 'chat' }}
                </span>
                <template v-for="mod in getInputModalities(remoteModel)" :key="mod">
                  <span class="modality-tag">{{ mod }}</span>
                </template>
              </div>
              <span class="remote-context">
                {{ formatContextLength(remoteModel.context_length) }}
              </span>
              <span v-if="formatMtokenPrice(remoteModel.pricing)" class="remote-price">
                {{ formatPriceDisplay(remoteModel.pricing) }}
              </span>
              <span v-else class="remote-price placeholder">N/A</span>
              <ElButton
                size="small"
                :type="
                  currentProviderForModels.enabled_models?.some((m) => m.id === remoteModel.id)
                    ? 'primary'
                    : 'default'
                "
                :disabled="
                  currentProviderForModels.enabled_models?.some((m) => m.id === remoteModel.id)
                "
                @click="addModelFromRemote(currentProviderForModels.provider_id, remoteModel)"
              >
                <ArtSvgIcon
                  v-if="
                    currentProviderForModels.enabled_models?.some((m) => m.id === remoteModel.id)
                  "
                  icon="ri:checkbox-circle-line"
                />
                <ArtSvgIcon v-else icon="ri:add-line" />
              </ElButton>
            </div>
          </div>
        </div>
      </div>
    </ElDialog>

    <!-- 模型配置弹窗 -->
    <ElDialog
      v-model="showModelModal"
      width="520px"
      destroy-on-close
      :close-on-click-modal="true"
      :show-close="false"
      class="extension-dialog model-provider-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
      footer-class="extension-dialog-footer"
    >
      <template #header>
        <ExtensionDialogHeader
          :title="isCreating ? '手动添加模型' : '模型配置'"
          @close="showModelModal = false"
        />
      </template>

      <div class="modal-form">
        <div v-if="isCreating" class="form-row">
          <label class="form-label full-width">
            <span>模型 ID <span class="required-mark">*</span></span>
            <ElInput v-model="editingModel.id" placeholder="例如 BAAI/bge-m3" clearable />
          </label>
        </div>
        <div v-else class="model-id-display">
          <span class="info-label">模型 ID</span>
          <code>{{ editingModel.id }}</code>
        </div>

        <div class="form-row">
          <label class="form-label">
            <span>展示名称</span>
            <ElInput v-model="editingModel.display_name" />
          </label>
          <label class="form-label">
            <span>模型类型</span>
            <ElSelect
              v-model="editingModel.type"
              :disabled="editingModelTypeOptions.length === 1"
              style="width: 100%"
            >
              <ElOption
                v-for="opt in editingModelTypeOptions"
                :key="opt.value"
                :value="opt.value"
                :label="opt.label"
              />
            </ElSelect>
          </label>
        </div>

        <div class="form-row">
          <label class="form-label">
            <span>协议覆盖</span>
            <ElInput v-model="editingModel.protocol_override" placeholder="可选" />
          </label>
          <label class="form-label">
            <span>Base URL 覆盖</span>
            <ElInput v-model="editingModel.base_url_override" placeholder="可选" />
          </label>
        </div>

        <div class="form-row">
          <label v-if="editingModel.type === 'embedding'" class="form-label">
            <span>维度</span>
            <ElInputNumber v-model="editingModel.dimension" :min="1" style="width: 100%" />
          </label>
          <label
            v-if="editingModel.type === 'embedding' || editingModel.type === 'rerank'"
            class="form-label"
          >
            <span>Batch Size</span>
            <ElInputNumber v-model="editingModel.batch_size" :min="1" style="width: 100%" />
          </label>
        </div>
      </div>

      <template #footer>
        <ElButton @click="showModelModal = false">取消</ElButton>
        <ElButton type="primary" :loading="saving" @click="saveModelConfig">确认</ElButton>
      </template>
    </ElDialog>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import {
    modelProviderApi,
    type EnabledModel,
    type ModelProvider,
    type ModelStatusResult
  } from '@/api/model-provider'
  import ExtensionCardGrid from '@/components/extensions/common/ExtensionCardGrid.vue'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import ExtensionEmptyState from '@/components/extensions/common/ExtensionEmptyState.vue'
  import ExtensionInfoCard, {
    type ExtensionCardTag
  } from '@/components/extensions/common/ExtensionInfoCard.vue'
  import ExtensionToolbar from '@/components/extensions/common/ExtensionToolbar.vue'
  import { unwrapApiData, unwrapList } from '@/utils/apiData'
  import { getProviderIcon } from '@/utils/modelIcon'

  const loading = ref(false)
  const remoteLoading = ref(false)
  const saving = ref(false)
  const togglingProviderId = ref<string | null>(null)
  const providers = ref<ModelProvider[]>([])
  const searchQuery = ref('')
  const modelTestLoadingBySpec = ref<Record<string, boolean>>({})
  const modelTestResultBySpec = ref<Record<string, ModelStatusResult>>({})

  // 供应商表单状态
  const showProviderModal = ref(false)
  const editingProviderId = ref<string | null>(null)
  const providerForm = reactive({
    provider_id: '',
    display_name: '',
    provider_type: 'openai',
    default_protocol: 'openai_compatible',
    base_url: '',
    embedding_base_url: '',
    rerank_base_url: '',
    models_endpoint: '/models',
    embedding_models_endpoint: '/embeddings/models',
    rerank_models_endpoint: '',
    api_key_env: '',
    api_key: '',
    capabilities: ['chat'] as string[],
    is_enabled: true,
    headers_text: '{}',
    extra_text: '{}'
  })

  // 模型表单状态
  const showModelModal = ref(false)
  const isCreating = ref(false)
  const editingModel = ref<EnabledModel>({
    id: '',
    display_name: '',
    type: 'chat',
    source: 'remote',
    protocol_override: null,
    base_url_override: null,
    context_length: null,
    dimension: null,
    batch_size: null,
    supported_parameters: [],
    extra: {}
  })

  // 模型管理弹窗状态
  const showModelsModal = ref(false)
  const currentProviderForModels = ref<ModelProvider | null>(null)

  // 远端模型缓存（按 provider_id）
  const remoteModelsMap = ref<Record<string, EnabledModel[]>>({})
  const remoteModelsLoaded = ref<Record<string, boolean>>({})
  const remoteModelSearch = ref<Record<string, string>>({})
  const remoteModelTypeFilter = ref<Record<string, string>>({})

  const filteredProviders = computed(() => {
    const keyword = searchQuery.value.trim().toLowerCase()
    const filtered = keyword
      ? providers.value.filter(
          (p) =>
            p.provider_id.toLowerCase().includes(keyword) ||
            p.display_name.toLowerCase().includes(keyword)
        )
      : providers.value
    return [...filtered].sort((a, b) => {
      if (a.is_enabled !== b.is_enabled) return a.is_enabled ? -1 : 1
      if (a.is_enabled && b.is_enabled && a.credential_status !== b.credential_status) {
        return a.credential_status === 'warning' ? 1 : -1
      }
      return a.provider_id.localeCompare(b.provider_id)
    })
  })

  const providerStats = computed(() => {
    let enabled = 0
    let warning = 0
    let models = 0
    for (const p of providers.value) {
      if (p.is_enabled) {
        enabled++
        if (p.credential_status === 'warning') warning++
      }
      models += p.enabled_models?.length || 0
    }
    return { total: providers.value.length, enabled, warning, models }
  })

  const remoteIdsMap = computed(() => {
    const map: Record<string, Set<string>> = {}
    for (const [providerId, models] of Object.entries(remoteModelsMap.value)) {
      map[providerId] = new Set(models.map((m) => m.id))
    }
    return map
  })

  const filteredRemoteModels = computed(() => {
    if (!currentProviderForModels.value) return []
    const providerId = currentProviderForModels.value.provider_id
    const query = (remoteModelSearch.value[providerId] || '').trim().toLowerCase()
    const typeFilter = remoteModelTypeFilter.value[providerId] || 'all'
    const models = remoteModelsMap.value[providerId] || []
    return models.filter((m) => {
      const matchesType = typeFilter === 'all' || (m.type || 'chat') === typeFilter
      const matchesQuery = !query || m.id.toLowerCase().includes(query)
      return matchesType && matchesQuery
    })
  })

  const remoteModelTypeOptions = computed(() => {
    if (!currentProviderForModels.value) return [{ label: '全部', value: 'all' }]
    const providerId = currentProviderForModels.value.provider_id
    const models = remoteModelsMap.value[providerId] || []
    const counts = models.reduce<Record<string, number>>((acc, model) => {
      const type = model.type || 'chat'
      acc[type] = (acc[type] || 0) + 1
      return acc
    }, {})
    return [
      { label: `全部 ${models.length}`, value: 'all' },
      { label: `Chat ${counts.chat || 0}`, value: 'chat' },
      { label: `Embedding ${counts.embedding || 0}`, value: 'embedding' },
      { label: `Rerank ${counts.rerank || 0}`, value: 'rerank' }
    ]
  })

  const editingModelTypeOptions = computed(() => {
    const caps = currentProviderForModels.value?.capabilities
    const types = Array.isArray(caps) && caps.length ? caps : ['chat', 'embedding', 'rerank']
    return types.map((c) => ({ value: c, label: c }))
  })

  const formatContextLength = (len?: number | null) => {
    if (!len) return '-'
    if (len >= 1000000) return `${(len / 1000000).toFixed(1)}M`
    if (len >= 1000) return `${(len / 1000).toFixed(0)}K`
    return len.toString()
  }

  const formatMtokenPrice = (pricing?: Record<string, unknown> | null) => {
    if (!pricing) return null
    const prompt = parseFloat(String(pricing.prompt || pricing.prompt_price || 0))
    const completion = parseFloat(String(pricing.completion || pricing.completion_price || 0))
    if (prompt < 0 || completion < 0) return null
    if (prompt === 0 && completion === 0) return null
    return {
      prompt: prompt * 100000,
      completion: completion * 100000
    }
  }

  const formatPriceDisplay = (pricing?: Record<string, unknown> | null) => {
    const p = formatMtokenPrice(pricing)
    if (!p) return null
    return `$${p.prompt.toFixed(2)} / $${p.completion.toFixed(2)}`
  }

  const getModelDisplayName = (model: EnabledModel) =>
    model.name || model.display_name || model.id

  const getModelId = (model: EnabledModel) => model.id

  const buildModelSpec = (providerId: string, modelId: string) => `${providerId}:${modelId}`

  const isModelTesting = (providerId: string, modelId: string) =>
    !!modelTestLoadingBySpec.value[buildModelSpec(providerId, modelId)]

  const getModelTestTitle = (providerId: string, model: EnabledModel) => {
    const spec = buildModelSpec(providerId, model.id)
    const result = modelTestResultBySpec.value[spec]
    if (!result) return '测试连接'
    const statusText =
      {
        available: '可用',
        unavailable: '不可用',
        unsupported: '暂不支持',
        error: '错误'
      }[result.status] || '未知'
    return `${statusText}: ${result.message || '无详细信息'}`
  }

  const getInputModalities = (model: EnabledModel) => {
    if (!model) return []
    if (model.input_modalities) return model.input_modalities
    if (model.architecture?.input_modalities) {
      return model.architecture.input_modalities as string[]
    }
    if (model.raw_metadata?.architecture) {
      const arch = model.raw_metadata.architecture as Record<string, unknown>
      if (Array.isArray(arch.input_modalities)) return arch.input_modalities as string[]
    }
    return []
  }

  const isModelStale = (model: EnabledModel, providerId: string) => {
    if (model.source === 'manual') return false
    if (!remoteModelsLoaded.value[providerId]) return false
    return model.enabled && !remoteIdsMap.value[providerId]?.has(model.id)
  }

  const capabilityLabels: Record<string, string> = {
    chat: 'Chat',
    embedding: 'Embedding',
    rerank: 'Rerank'
  }

  const capabilityColors: Record<string, ExtensionCardTag['color']> = {
    chat: 'blue',
    embedding: 'purple',
    rerank: 'cyan'
  }

  const getProviderTags = (provider: ModelProvider): ExtensionCardTag[] => {
    const tags: ExtensionCardTag[] = []
    if (!provider.is_enabled) {
      tags.push({ name: '未启用', color: 'orange' })
    } else if (provider.credential_status === 'warning') {
      tags.push({ name: '凭证缺失', color: 'orange' })
    } else {
      tags.push({ name: '已启用', color: 'green' })
    }

    const caps = (provider.capabilities?.length ? provider.capabilities : ['chat']).slice(0, 3)
    for (const cap of caps) {
      const key = String(cap).toLowerCase()
      tags.push({
        name: capabilityLabels[key] || String(cap),
        color: capabilityColors[key] || 'blue'
      })
    }
    return tags
  }

  const parseJsonObject = (text: string, label: string) => {
    try {
      const parsed = JSON.parse(text || '{}')
      if (!parsed || Array.isArray(parsed) || typeof parsed !== 'object') {
        throw new Error(`${label} 必须是 JSON 对象`)
      }
      return parsed
    } catch {
      throw new Error(`${label} 格式不正确`)
    }
  }

  const formatJsonText = (value?: Record<string, unknown> | null) =>
    JSON.stringify(value || {}, null, 2)

  const loadProviders = async () => {
    loading.value = true
    try {
      const result = await modelProviderApi.getProviders()
      providers.value = unwrapList<ModelProvider>(result)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载模型供应商失败')
    } finally {
      loading.value = false
    }
  }

  const openCreateProviderModal = () => {
    editingProviderId.value = null
    Object.assign(providerForm, {
      provider_id: '',
      display_name: '',
      provider_type: 'openai',
      default_protocol: '',
      base_url: '',
      embedding_base_url: '',
      rerank_base_url: '',
      models_endpoint: '/models',
      embedding_models_endpoint: '/embeddings/models',
      rerank_models_endpoint: '',
      api_key_env: '',
      api_key: '',
      capabilities: ['chat'],
      is_enabled: true,
      headers_text: '{}',
      extra_text: '{}'
    })
    showProviderModal.value = true
  }

  const openEditProviderModal = (provider: ModelProvider) => {
    editingProviderId.value = provider.provider_id
    Object.assign(providerForm, {
      provider_id: provider.provider_id,
      display_name: provider.display_name,
      provider_type: provider.provider_type || 'openai',
      default_protocol: '',
      base_url: provider.base_url || '',
      embedding_base_url: provider.embedding_base_url || '',
      rerank_base_url: provider.rerank_base_url || '',
      models_endpoint: provider.models_endpoint ?? '',
      embedding_models_endpoint: provider.embedding_models_endpoint ?? '',
      rerank_models_endpoint: provider.rerank_models_endpoint ?? '',
      api_key_env: provider.api_key_env || '',
      api_key: provider.api_key || '',
      capabilities: provider.capabilities?.length ? provider.capabilities : ['chat'],
      is_enabled: provider.is_enabled !== false,
      headers_text: formatJsonText(provider.headers_json),
      extra_text: formatJsonText(provider.extra_json)
    })
    showProviderModal.value = true
  }

  const buildProviderPayload = () => ({
    provider_id: providerForm.provider_id || undefined,
    display_name: providerForm.display_name,
    provider_type: providerForm.provider_type,
    default_protocol: null,
    base_url: providerForm.base_url,
    embedding_base_url: providerForm.embedding_base_url || null,
    rerank_base_url: providerForm.rerank_base_url || null,
    models_endpoint: providerForm.models_endpoint || null,
    embedding_models_endpoint: providerForm.embedding_models_endpoint || null,
    rerank_models_endpoint: providerForm.rerank_models_endpoint || null,
    api_key_env: providerForm.api_key_env || null,
    api_key: providerForm.api_key || null,
    capabilities: providerForm.capabilities,
    is_enabled: providerForm.is_enabled,
    headers_json: parseJsonObject(providerForm.headers_text, '请求头'),
    extra_json: parseJsonObject(providerForm.extra_text, '扩展配置')
  })

  const createProvider = async () => {
    saving.value = true
    try {
      await modelProviderApi.createProvider(buildProviderPayload())
      ElMessage.success('供应商已创建')
      showProviderModal.value = false
      await loadProviders()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '创建失败')
    } finally {
      saving.value = false
    }
  }

  const saveProvider = async () => {
    saving.value = true
    try {
      await modelProviderApi.updateProvider(providerForm.provider_id, buildProviderPayload())
      ElMessage.success('供应商已保存')
      showProviderModal.value = false
      await loadProviders()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '保存失败')
    } finally {
      saving.value = false
    }
  }

  const deleteProvider = async (provider: ModelProvider) => {
    try {
      await ElMessageBox.confirm(
        '删除后不会影响当前系统正在使用的旧模型配置。',
        `删除 ${provider.display_name}`,
        {
          type: 'warning',
          confirmButtonText: '删除',
          cancelButtonText: '取消',
          confirmButtonClass: 'el-button--danger'
        }
      )
      await modelProviderApi.deleteProvider(provider.provider_id)
      ElMessage.success('已删除')
      if (currentProviderForModels.value?.provider_id === provider.provider_id) {
        showModelsModal.value = false
        currentProviderForModels.value = null
      }
      if (editingProviderId.value === provider.provider_id) {
        showProviderModal.value = false
        editingProviderId.value = null
      }
      await loadProviders()
    } catch {
      // 用户取消或请求失败（拦截器已提示）
    }
  }

  const deleteProviderFromEdit = async () => {
    const provider = providers.value.find((p) => p.provider_id === editingProviderId.value)
    if (provider) await deleteProvider(provider)
  }

  const toggleProviderEnabled = async (provider: ModelProvider, checked: boolean) => {
    togglingProviderId.value = provider.provider_id
    try {
      await modelProviderApi.updateProvider(provider.provider_id, { is_enabled: checked })
      ElMessage.success(checked ? '已启用' : '已停用')
      await loadProviders()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '操作失败')
      await loadProviders()
    } finally {
      togglingProviderId.value = null
    }
  }

  const openModelsModal = (provider: ModelProvider) => {
    currentProviderForModels.value = provider
    if (!remoteModelsLoaded.value[provider.provider_id]) {
      remoteModelsMap.value[provider.provider_id] = []
    }
    remoteModelSearch.value[provider.provider_id] =
      remoteModelSearch.value[provider.provider_id] || ''
    remoteModelTypeFilter.value[provider.provider_id] = 'all'
    showModelsModal.value = true
  }

  const fetchRemoteModels = async (providerId: string) => {
    remoteLoading.value = true
    try {
      const result = await modelProviderApi.fetchRemoteModels(providerId)
      const models = unwrapList<EnabledModel>(result)
      remoteModelsMap.value = { ...remoteModelsMap.value, [providerId]: models }
      remoteModelsLoaded.value[providerId] = true
      ElMessage.success(`已获取 ${models.length} 个远端模型`)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '获取远端模型失败')
    } finally {
      remoteLoading.value = false
    }
  }

  const normalizeModel = (model: EnabledModel = { id: '' }): EnabledModel => ({
    id: model.id || '',
    display_name: model.display_name || model.name || model.id || '',
    type: model.type && model.type !== 'unknown' ? model.type : 'chat',
    source: model.source || 'remote',
    protocol_override: model.protocol_override || null,
    base_url_override: model.base_url_override || null,
    context_length: model.context_length || null,
    dimension: model.dimension || null,
    batch_size: model.batch_size || null,
    supported_parameters: model.supported_parameters || [],
    extra: model.extra || {}
  })

  const testModelConnection = async (providerId: string, model: EnabledModel) => {
    const spec = buildModelSpec(providerId, model.id)
    if (modelTestLoadingBySpec.value[spec]) return

    modelTestLoadingBySpec.value = { ...modelTestLoadingBySpec.value, [spec]: true }
    try {
      const result = await modelProviderApi.getModelStatusBySpec(spec)
      const status = unwrapApiData<ModelStatusResult>(result, {
        spec,
        status: 'error',
        message: '检查失败'
      })
      modelTestResultBySpec.value = { ...modelTestResultBySpec.value, [spec]: status }

      if (status.status === 'available') {
        ElMessage.success(`${getModelDisplayName(model)} 连接正常`)
      } else if (status.status === 'unsupported') {
        ElMessage.warning(status.message || '暂不支持测试该类型模型')
      } else if (status.status === 'unavailable') {
        ElMessage.warning(status.message || '模型连接不可用')
      } else {
        ElMessage.error(status.message || '模型连接测试失败')
      }
    } catch (error) {
      modelTestResultBySpec.value = {
        ...modelTestResultBySpec.value,
        [spec]: { spec, status: 'error', message: (error as Error)?.message || '检查失败' }
      }
      ElMessage.error((error as Error)?.message || '模型连接测试失败')
    } finally {
      modelTestLoadingBySpec.value = { ...modelTestLoadingBySpec.value, [spec]: false }
    }
  }

  const addModelFromRemote = async (providerId: string, remoteModel: EnabledModel) => {
    const provider = providers.value.find((p) => p.provider_id === providerId)
    if (!provider) return

    const enabledModels = provider.enabled_models || []
    if (enabledModels.some((m) => m.id === remoteModel.id)) {
      ElMessage.info('模型已存在')
      return
    }

    const newModel = normalizeModel(remoteModel)
    newModel.source = 'remote'
    newModel.enabled = true
    const newEnabledModels = [...enabledModels, newModel]

    try {
      await modelProviderApi.updateProvider(providerId, { enabled_models: newEnabledModels })
      ElMessage.success(`已添加模型 ${remoteModel.id}`)
      await loadProviders()
      if (currentProviderForModels.value?.provider_id === providerId) {
        currentProviderForModels.value =
          providers.value.find((p) => p.provider_id === providerId) || null
      }
    } catch (error) {
      ElMessage.error((error as Error)?.message || '添加模型失败')
    }
  }

  const openModelConfigModal = (model: EnabledModel) => {
    editingModel.value = normalizeModel(model)
    isCreating.value = false
    showModelModal.value = true
  }

  const openCreateModal = (provider: ModelProvider) => {
    if (!provider) return
    const types = provider.capabilities?.length ? provider.capabilities : ['chat']
    const defaultType = types[0]
    editingModel.value = {
      id: '',
      display_name: '',
      type: defaultType,
      source: 'manual',
      protocol_override: null,
      base_url_override: null,
      context_length: null,
      dimension: null,
      batch_size: null,
      supported_parameters: [],
      extra: {}
    }
    isCreating.value = true
    showModelModal.value = true
  }

  const saveModelConfig = async () => {
    if (!currentProviderForModels.value) return
    saving.value = true
    try {
      const provider = providers.value.find(
        (p) => p.provider_id === currentProviderForModels.value?.provider_id
      )
      if (!provider) return

      let enabledModels: EnabledModel[]
      if (isCreating.value) {
        const newId = (editingModel.value.id || '').trim()
        if (!newId) {
          ElMessage.error('请填写模型 ID')
          return
        }
        if ((provider.enabled_models || []).some((m) => m.id === newId)) {
          ElMessage.error('模型 ID 已存在')
          return
        }
        const newModel: EnabledModel = {
          ...editingModel.value,
          id: newId,
          source: 'manual',
          enabled: true
        }
        enabledModels = [...(provider.enabled_models || []), newModel]
      } else {
        enabledModels = (provider.enabled_models || []).map((m) =>
          m.id === editingModel.value.id ? { ...editingModel.value } : m
        )
      }

      await modelProviderApi.updateProvider(currentProviderForModels.value.provider_id, {
        enabled_models: enabledModels
      })
      ElMessage.success(isCreating.value ? '模型已添加' : '模型配置已保存')
      showModelModal.value = false
      isCreating.value = false
      await loadProviders()
      currentProviderForModels.value =
        providers.value.find(
          (p) => p.provider_id === currentProviderForModels.value?.provider_id
        ) || null
    } catch (error) {
      ElMessage.error((error as Error)?.message || '保存失败')
    } finally {
      saving.value = false
    }
  }

  const removeModel = async (providerId: string, modelId: string) => {
    const provider = providers.value.find((p) => p.provider_id === providerId)
    if (!provider) return

    try {
      await ElMessageBox.confirm(`确定要移除模型 ${modelId} 吗？`, '移除模型', {
        type: 'warning',
        confirmButtonText: '移除',
        cancelButtonText: '取消',
        confirmButtonClass: 'el-button--danger'
      })
      const enabledModels = (provider.enabled_models || []).filter((m) => m.id !== modelId)
      await modelProviderApi.updateProvider(providerId, { enabled_models: enabledModels })
      ElMessage.success('模型已移除')
      await loadProviders()
      if (currentProviderForModels.value?.provider_id === providerId) {
        currentProviderForModels.value =
          providers.value.find((p) => p.provider_id === providerId) || null
      }
    } catch {
      // 用户取消或请求失败
    }
  }

  onMounted(loadProviders)

  defineExpose({
    loading,
    stats: providerStats,
    refresh: loadProviders
  })
</script>

<style scoped lang="scss">
  .model-provider-manage-panel {
    height: 100%;
    min-height: 0;

    :deep(.provider-card-grid) {
      gap: 12px;
    }

    @media (min-width: 1080px) {
      :deep(.provider-card-grid) {
        grid-template-columns: repeat(4, minmax(0, 1fr));
      }
    }

    :deep(.provider-info-card .card-shell) {
      padding-bottom: 2px;
      background: linear-gradient(
        to left,
        color-mix(in srgb, var(--el-color-primary-light-9) 92%, var(--el-bg-color)) 0%,
        color-mix(in srgb, var(--el-color-primary-light-9) 55%, var(--el-bg-color)) 42%,
        var(--el-bg-color) 100%
      );
    }

    :deep(.card-bottom) {
      justify-content: flex-start;
    }

    :deep(.card-tags) {
      flex: initial;
      width: 100%;
    }

    :deep(.provider-info-card .card-toolbar) {
      width: 100%;
      margin-top: 0;
      padding-top: 2px;
      padding-bottom: 0;
    }

    :deep(.provider-info-card .card-bottom) {
      min-height: 0;
    }
  }

  .provider-desc-line {
    margin: 0;
    min-height: calc(1.4em * 1);
    font-size: 12px;
    line-height: 1.4;
    color: var(--el-text-color-regular);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .provider-desc-url {
    color: var(--el-text-color-secondary);
  }

  .provider-card-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    gap: 8px;
    min-height: 22px;
  }

  .view-models-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 2px;
    font-size: 12px;
    font-weight: 500;
    color: var(--el-text-color-regular);
  }

  .view-models-icon {
    margin-right: 2px;
    font-size: 17px;
    line-height: 1;
  }

  .enabled-count {
    margin-left: 2px;
    color: var(--el-text-color-secondary);
    font-size: 11px;
    font-weight: 400;
    line-height: 1;
    white-space: nowrap;
  }

  .spinning {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .models-modal-content {
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .models-section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .models-section-title {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--el-text-color-regular);
  }

  .models-table {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 6px;
    overflow: hidden;
  }

  .table-head,
  .table-row {
    display: grid;
    grid-template-columns: 1fr 80px 70px 60px 150px;
    gap: 8px;
    align-items: center;
  }

  .table-head {
    padding: 10px 12px;
    background: var(--el-fill-color-light);
    font-size: 11px;
    font-weight: 600;
    color: var(--el-text-color-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .table-row {
    padding: 10px 12px;
    border-top: 1px solid var(--el-border-color-extra-light);
    font-size: 13px;
    transition: background 0.1s;

    &:hover {
      background: var(--el-fill-color-lighter);
    }

    &.stale {
      background: var(--el-color-warning-light-9);
    }
  }

  .model-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .model-name {
    font-weight: 500;
    color: var(--el-text-color-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .model-id {
    font-size: 11px;
    color: var(--el-text-color-secondary);
    font-family: monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .col-type {
    display: flex;
    align-items: center;
  }

  .col-context,
  .col-dim {
    color: var(--el-text-color-regular);
    font-size: 12px;
  }

  .col-ops {
    display: flex;
    gap: 4px;
    justify-content: flex-end;
  }

  .model-test-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    min-width: 28px;
    padding: 0;
    color: var(--el-color-primary);

    &.is-testing {
      color: var(--el-color-primary-light-3);
      cursor: wait;
    }
  }

  .type-tag {
    display: inline-flex;
    align-items: center;
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 500;

    & + & {
      margin-left: 4px;
    }

    &.chat {
      background: var(--el-color-primary-light-9);
      color: var(--el-color-primary);
    }

    &.embedding {
      background: var(--el-color-success-light-9);
      color: var(--el-color-success);
    }

    &.rerank {
      background: var(--el-color-warning-light-9);
      color: var(--el-color-warning);
    }

    &.manual {
      background: var(--el-fill-color);
      color: var(--el-text-color-regular);
    }
  }

  .enabled-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;

    .actions {
      display: flex;
      align-items: center;
      gap: 8px;
    }
  }

  .remote-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }

  .remote-search-input {
    width: 180px;
  }

  .remote-type-filter {
    flex-shrink: 0;
  }

  .remote-list {
    border-top: 1px solid var(--el-border-color-extra-light);
  }

  .remote-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 0;
    border-top: 1px solid var(--el-border-color-extra-light);
    font-size: 13px;

    &:first-child {
      border-top: none;
    }
  }

  .remote-name {
    flex: 1;
    min-width: 0;
    font-weight: 500;
    color: var(--el-text-color-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .remote-tags {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .modality-tag {
    display: inline-flex;
    align-items: center;
    padding: 2px 6px;
    border-radius: 3px;
    background: var(--el-color-info-light-9);
    color: var(--el-color-info);
    font-size: 10px;
    font-weight: 500;
  }

  .dim-warning {
    display: inline-flex;
    align-items: center;
    padding: 2px 6px;
    border-radius: 3px;
    background: var(--el-color-warning-light-9);
    color: var(--el-color-warning);
    font-size: 10px;
    font-weight: 500;
  }

  .remote-context {
    width: 60px;
    color: var(--el-text-color-secondary);
    font-size: 12px;
  }

  .remote-price {
    width: 100px;
    font-size: 11px;
    color: var(--el-text-color-regular);
    font-family: monospace;

    &.placeholder {
      color: var(--el-text-color-placeholder);
    }
  }

  .modal-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .form-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }

  .form-label {
    display: flex;
    flex-direction: column;
    gap: 6px;

    > span {
      color: var(--el-text-color-regular);
      font-size: 12px;
      font-weight: 500;
    }
  }

  .full-width {
    grid-column: 1 / -1;
  }

  .required-mark {
    color: var(--el-color-danger);
  }

  .form-switch {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 0;

    > span {
      color: var(--el-text-color-regular);
      font-size: 12px;
      font-weight: 500;
    }
  }

  .advanced-collapse {
    :deep(.el-collapse-item__content) {
      display: flex;
      flex-direction: column;
      gap: 14px;
      padding-bottom: 0;
    }
  }

  .provider-modal-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
  }

  .provider-modal-footer-actions {
    display: flex;
    gap: 8px;
  }

  .model-id-display {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    background: var(--el-fill-color-light);
    border-radius: 6px;
    margin-bottom: 4px;

    .info-label {
      color: var(--el-text-color-secondary);
      font-size: 11px;
      font-weight: 500;
      text-transform: uppercase;
    }

    code {
      font-family: monospace;
      font-size: 13px;
      color: var(--el-text-color-primary);
    }
  }

  @media (max-width: 768px) {
    .form-row {
      grid-template-columns: 1fr;
    }

    .table-head,
    .table-row {
      grid-template-columns: 1fr 60px 60px;
      font-size: 12px;
    }

    .col-context,
    .col-dim {
      display: none;
    }
  }
</style>
