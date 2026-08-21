<!-- 聊天页（基于 vue-element-plus-x 组件库重构） -->
<template>
  <div
    class="page-content flex !p-0 max-md:flex-col relative"
    :style="{ height: containerMinHeight }"
  >
    <!-- 左侧：会话列表 -->
    <div
      class="sidebar-container box-border h-full p-5 border-r border-g-300 max-md:w-full max-md:h-42 max-md:border-r-0 transition-all duration-300 overflow-hidden shrink-0 flex flex-col"
      :style="{ width: sidebarCollapsed ? '0px' : '280px' }"
      :class="sidebarCollapsed ? '!p-0 !border-r-0' : ''"
    >
      <div class="pb-5 max-md:!hidden">
        <div class="flex-c gap-3">
          <ElAvatar :size="50" :src="userAvatarSrc">{{ userName.charAt(0) }}</ElAvatar>
          <div class="flex-1 min-w-0">
            <div class="text-base font-medium">{{ userName }}</div>
            <div class="mt-1 text-xs text-g-500">{{ userEmail }}</div>
          </div>
          <ElButton circle :icon="Plus" @click="createNewChat" title="新建对话" />
        </div>
        <div class="mt-3">
          <ElInput v-model="searchQuery" placeholder="搜索对话" prefix-icon="Search" clearable />
        </div>
      </div>
      <ElScrollbar @scroll="onThreadListScroll" class="flex-1 min-h-0">
        <div
          v-for="thread in filteredThreads"
          :key="thread.id"
          class="thread-row flex-c p-3 c-p rounded-lg tad-200 hover:bg-active-color/30 mb-1"
          :class="{ 'bg-active-color': currentThreadId === thread.id }"
          @click="switchThread(thread.id)"
          @dblclick="renameThread(thread)"
        >
          <div class="relative mr-3">
            <ElAvatar
              :size="40"
              class="thread-gradient-avatar"
              :style="getThreadAvatarStyle(thread)"
              :aria-label="`${thread.title || '新的对话'}头像`"
            />
            <div
              class="absolute right-1 bottom-1 size-2 rounded-full"
              :class="
                currentThreadId === thread.id
                  ? isStreaming
                    ? 'bg-warning'
                    : 'bg-success'
                  : 'bg-g-300'
              "
            ></div>
          </div>
          <div class="flex-1 min-w-0 flex flex-col justify-center">
            <span class="text-sm font-medium truncate">{{ thread.title || '新的对话' }}</span>
            <div class="flex items-center justify-between gap-2 mt-0.5">
              <span class="text-xs text-g-500 shrink-0">{{
                formatThreadTime(thread.updated_at)
              }}</span>
              <div class="flex items-center gap-0 ml-auto">
                <ElButton
                  class="thread-action-btn"
                  :icon="EditPen"
                  circle
                  size="small"
                  text
                  title="重命名"
                  @click.stop="renameThread(thread)"
                />
                <ElButton
                  class="thread-action-btn"
                  :icon="Top"
                  circle
                  size="small"
                  text
                  :type="thread.is_pinned ? 'primary' : ''"
                  title="置顶"
                  @click.stop="togglePinThread(thread)"
                />
                <ElButton
                  class="thread-action-btn"
                  :icon="Delete"
                  circle
                  size="small"
                  text
                  title="删除"
                  @click.stop="deleteThread(thread.id)"
                />
              </div>
            </div>
          </div>
        </div>
        <div v-if="!filteredThreads.length" class="text-center text-g-500 py-5 text-sm">
          暂无对话
        </div>
        <div
          v-if="chatThreadsStore.isLoadingMoreThreads"
          class="text-center text-g-400 py-3 text-xs"
        >
          加载更多...
        </div>
        <div
          v-else-if="!chatThreadsStore.hasMoreThreads && filteredThreads.length"
          class="text-center text-g-400 py-3 text-xs"
        >
          没有更多了
        </div>
      </ElScrollbar>
    </div>

    <!-- 折叠按钮 -->
    <button
      class="sidebar-toggle absolute top-1/2 -translate-y-1/2"
      :style="{ left: sidebarCollapsed ? '0px' : '258px' }"
      @click="sidebarCollapsed = !sidebarCollapsed"
      :title="sidebarCollapsed ? '展开会话列表' : '折叠会话列表'"
    >
      <ArtSvgIcon
        :icon="sidebarCollapsed ? 'ri:arrow-right-s-line' : 'ri:arrow-left-s-line'"
        class="text-lg"
      />
    </button>

    <!-- 右侧：对话区 -->
    <div class="box-border flex-1 h-full max-md:h-[calc(70%-30px)] flex flex-col">
      <!-- 对话标题栏 -->
      <div class="pt-3 px-4 pb-2 flex-cb shrink-0">
        <ElDropdown trigger="click" placement="bottom-start" @command="handleAgentCommand">
          <div class="flex-c gap-2 c-p">
            <img
              v-if="agentAvatarSrc"
              :src="agentAvatarSrc"
              class="agent-dropdown-avatar"
              :alt="`${currentAgentName}头像`"
            />
            <span class="text-base font-medium">{{ currentAgentName }}</span>
            <ArtSvgIcon icon="ri:arrow-down-s-line" class="text-sm text-g-500" />
          </div>
          <template #dropdown>
            <ElDropdownMenu>
              <ElDropdownItem
                v-for="agent in agentStore.agents"
                :key="agent.id"
                :command="{ action: 'select', agentId: agent.id }"
                :disabled="
                  currentThreadId && currentThreadAgentId && agent.id !== currentThreadAgentId
                "
              >
                <div class="agent-dropdown-item">
                  <img
                    :src="getAgentAvatarSrc(agent)"
                    class="agent-dropdown-avatar"
                    :alt="`${agent.name || agent.id}头像`"
                  />
                  <span class="agent-dropdown-name">{{ agent.name || agent.id }}</span>
                  <ArtSvgIcon
                    v-if="agent.id === agentStore.selectedAgentId"
                    icon="ri:check-line"
                    class="ml-auto shrink-0"
                  />
                </div>
              </ElDropdownItem>
              <ElDropdownItem divided :command="{ action: 'manage' }">
                <div class="agent-dropdown-item">
                  <ArtSvgIcon icon="ri:settings-3-line" class="mr-2" />
                  <span>管理智能体</span>
                </div>
              </ElDropdownItem>
            </ElDropdownMenu>
          </template>
        </ElDropdown>
        <div class="flex-c gap-1">
          <button
            class="header-panel-btn"
            :class="{ active: statePanelOpen }"
            title="状态面板"
            @click="toggleStatePanel"
          >
            <ArtSvgIcon icon="ri:bar-chart-2-line" class="text-base" />
          </button>
          <button
            v-if="supportsFiles"
            class="header-panel-btn"
            :class="{ active: fileWorkspaceOpen }"
            title="文件工作区"
            @click="toggleFileWorkspace"
          >
            <ArtSvgIcon icon="ri:folder-open-line" class="text-base" />
          </button>
        </div>
      </div>

      <!-- 消息区 + 输入区 -->
      <div class="flex-1 min-h-0 flex flex-col px-4 pb-2 relative">
        <!-- 消息列表：始终 flex + min-h-0，保证 StickToBottom 拿到确定高度才能内部滚动 -->
        <div
          class="chat-conversation-region flex-1 min-h-0 flex flex-col overflow-hidden"
          :class="{ 'is-empty': isConversationEmpty }"
        >
          <div
            class="chat-ai-elements flex-1 min-h-0 flex flex-col overflow-hidden"
            :class="{ 'is-empty': isConversationEmpty }"
          >
            <ElementsConversation
              ref="elementsConversationRef"
              class="min-h-0 flex-1"
              :items="bubbleListItems"
              :is-streaming="isStreaming"
              :is-reply-loading="isReplyLoading"
              :is-loading-messages="isLoadingMessages"
              :greeting-text="greetingText"
              :starter-prompts="starterPrompts"
              :user-name="userName"
              :assistant-name="currentAgentName"
              :assistant-avatar="agentAvatarSrc"
              :thread-id="currentThreadId || ''"
              @copy="copyMessage"
              @retry="retryMessage"
              @prompt-click="handlePromptClick"
              @preview-image="({ mime, content }) => openImagePreview(mime, content)"
            />
          </div>
          <div v-if="isLoadingMessages" class="flex justify-center mt-10">
            <ElIcon class="is-loading" :size="24"><Loading /></ElIcon>
          </div>
        </div>

        <!-- 附件预览（已确认添加到线程的附件） -->
        <Attachments
          v-if="currentThreadAttachments.length"
          :items="attachmentCardItems"
          overflow="wrap"
          :hide-upload="true"
          class="chat-attachment-preview"
          @deleteCard="
            (item) => handleAttachmentRemove({ file_id: item.uid, file_name: item.name })
          "
        />

        <!-- 输入区：Elements PromptInput 外壳 + 现有 Mention 编辑器 -->
        <div
          class="chat-composer-container pt-2 shrink-0 px-4"
          :class="{ 'is-empty': isConversationEmpty }"
        >
          <div class="chat-input-wrapper">
            <ElementsPromptShell>
              <MessageInputComponent
                ref="messageInputRef"
                :model-value="inputText"
                :is-loading="isStreaming"
                :mention="mentionConfig"
                :thread-id="currentThreadId || ''"
                placeholder="描述设备现象、输入故障码，或使用 @ 引用资源…"
                @update:model-value="handleInputUpdate"
                @send="handleSendOrStop"
                @keydown="handleInputKeyDown"
              >
                <template #options-left>
                  <button
                    v-if="supportsFileUpload"
                    type="button"
                    class="input-attachment-btn"
                    :disabled="!supportsFileUpload"
                    @click="openAttachmentModal"
                    title="上传附件"
                  >
                    <ArtSvgIcon icon="ri:attachment-line" class="text-base" />
                  </button>
                </template>
                <template #actions-right>
                  <ModelSelectorComponent
                    :model_spec="currentModelSpec"
                    display-name="mini"
                    placeholder="默认模型"
                    @select-model="handleModelSelect"
                  />
                  <div class="actions-right-gap"></div>
                </template>
              </MessageInputComponent>
            </ElementsPromptShell>
          </div>
        </div>

        <!-- 状态面板：悬浮在聊天区右上角 -->
        <div
          id="agent-state-panel"
          class="side-panel side-panel--state is-floating"
          :class="{ 'is-visible': statePanelOpen }"
          :style="{ right: fileWorkspaceOpen ? `${filePanelWidth + 12}px` : '12px' }"
        >
          <div v-if="statePanelOpen" class="state-panel">
            <div class="state-panel-header">
              <span class="state-panel-title">状态</span>
              <div class="state-panel-header-actions">
                <span class="state-panel-summary">{{ stateSummaryLabel }}</span>
                <button
                  type="button"
                  class="state-refresh-btn"
                  title="刷新状态"
                  :disabled="isRefreshingState"
                  @click="handleAgentStateRefresh()"
                >
                  <ArtSvgIcon
                    icon="ri:refresh-line"
                    :class="{ 'is-spinning': isRefreshingState }"
                  />
                </button>
              </div>
            </div>
            <div class="state-panel-body">
              <div v-if="isRefreshingState && !currentAgentState" class="state-panel-empty">
                正在加载状态…
              </div>
              <div v-else-if="stateLoadError" class="state-panel-error" role="alert">
                <span>{{ stateLoadError }}</span>
                <button type="button" @click="handleAgentStateRefresh()">重试</button>
              </div>
              <div v-if="currentAgentState" class="state-overview" aria-label="状态概览">
                <div class="state-overview-item">
                  <strong>{{ stateTodos.length }}</strong>
                  <span>待办</span>
                </div>
                <div class="state-overview-item">
                  <strong>{{ stateFiles }}</strong>
                  <span>文件</span>
                </div>
                <div class="state-overview-item">
                  <strong>{{ currentArtifacts.length }}</strong>
                  <span>产物</span>
                </div>
                <div class="state-overview-item">
                  <strong>{{ stateSubagentRuns }}</strong>
                  <span>子任务</span>
                </div>
              </div>
              <AgentStatePanel
                v-if="currentAgentState && hasVisibleStateSections"
                :agent-state="currentAgentState"
                :thread-id="currentThreadId || ''"
              />
              <div
                v-if="currentAgentState && !hasVisibleStateSections && !isRefreshingState"
                class="state-panel-empty"
                >当前会话暂无待办、文件、产物或子任务</div
              >
              <div v-if="currentArtifacts.length" class="state-panel-artifacts">
                <ArtifactsCard
                  :artifacts="currentArtifacts"
                  :thread-id="currentThreadId"
                  @saved="
                    () => {
                      fileWorkspaceOpen = true
                    }
                  "
                />
              </div>
            </div>
          </div>
        </div>

        <!-- 文件工作区（右侧 side-panel） -->
        <div
          id="agent-file-panel"
          class="side-panel side-panel--file"
          :class="{ 'is-visible': fileWorkspaceOpen, 'is-docked': fileWorkspaceOpen }"
          :style="{ flexBasis: fileWorkspaceOpen ? `${filePanelWidth}px` : '0px' }"
        >
          <FileWorkspacePanel
            v-if="fileWorkspaceOpen && currentThreadId"
            :thread-id="currentThreadId"
            :visible="fileWorkspaceOpen"
            @close="fileWorkspaceOpen = false"
          />
          <div v-else-if="fileWorkspaceOpen && !currentThreadId" class="file-panel-empty"
            >请先选择一个对话</div
          >
        </div>
      </div>
    </div>

    <!-- 弹窗必须放在单一根节点内，否则 KeepAlive/Transition(out-in) 离开本页时会空白 -->
    <ApprovalCard
      v-if="approvalState.showModal && approvalState.threadId === currentThreadId"
      :questions="approvalState.questions"
      :status="approvalState.status"
      @submit="handleApprovalSubmit"
      @cancel="handleApprovalCancel"
      class="fixed left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 z-50"
    />

    <!-- 图片放大预览 -->
    <ElDialog
      v-model="imagePreviewVisible"
      :show-close="false"
      width="auto"
      align-center
      append-to-body
      class="image-preview-dialog"
      @click="imagePreviewVisible = false"
    >
      <img v-if="imagePreviewSrc" :src="imagePreviewSrc" alt="预览" class="image-preview-img" />
    </ElDialog>

    <AgentEditModal ref="agentEditModalRef" />
    <AttachmentTmpUploadModal
      v-model:open="attachmentUploadModalOpen"
      :thread-id="currentThreadId"
      :ensure-thread="ensureAttachmentThread"
      @added="handleTmpAttachmentsAdded"
    />
  </div>
</template>

<script setup>
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { Plus, Delete, Top, EditPen, Loading } from '@element-plus/icons-vue'
  import { Attachments } from 'vue-element-plus-x'
  import MessageInputComponent from '@/components/chat/MessageInputComponent.vue'
  import ElementsPromptShell from '@/components/chat/ElementsPromptShell.vue'
  import ElementsConversation from '@/components/chat/ElementsConversation.vue'
  import { agentApi } from '@/api/agent'
  import { threadApi } from '@/api/thread'
  import { usableMediaSrc } from '@/utils/mediaSrc'
  import { generatePixelAvatar } from '@/utils/pixelAvatar'
  import { createThreadGradientStyle } from '@/utils/threadGradientAvatar'
  import defaultUserAvatar from '@imgs/user/foxops-avatar.png'
  import { useUserStore } from '@/store/modules/user'
  import { useAgentStore } from '@/store/modules/agent'
  import { useChatThreadsStore } from '@/store/modules/chatThreads'
  import { useAgentThreadState } from '@/hooks/agent/useAgentThreadState'
  import { useStreamSmoother } from '@/hooks/agent/useStreamSmoother'
  import { useAgentStreamHandler } from '@/hooks/agent/useAgentStreamHandler'
  import { useAgentRunStream } from '@/hooks/agent/useAgentRunStream'
  import { MessageProcessor } from '@/utils/messageProcessor'
  import { getConversationDisplayItems } from '@/utils/messageGrouping'
  import { resolveRetryUserMessage } from '@/utils/chat/retryMessage'
  import { parseMentionText } from '@/utils/mention_utils'
  import {
    isMentionAgentResourceKind,
    isDefaultAllAgentResourceKind,
    getAgentConfigOptions,
    getAgentConfigOptionValue,
    getAgentConfigOptionLabel,
    getAgentConfigOptionDescription
  } from '@/utils/agentConfigUtils'
  import { useAutoLayoutHeight } from '@/hooks/core/useLayoutHeight'
  import AgentEditModal from '@/components/chat/AgentEditModal.vue'
  import ApprovalCard from '@/components/chat/ApprovalCard.vue'
  import ModelSelectorComponent from '@/components/chat/ModelSelectorComponent.vue'
  import AttachmentTmpUploadModal from '@/components/chat/AttachmentTmpUploadModal.vue'
  import AgentStatePanel from '@/components/chat/AgentStatePanel.vue'
  import FileWorkspacePanel from '@/components/chat/FileWorkspacePanel.vue'
  import ArtifactsCard from '@/components/chat/ArtifactsCard.vue'
  import { useApproval } from '@/hooks/agent/useApproval'
  import { useRoute, useRouter } from 'vue-router'
  import 'katex/dist/katex.min.css'

  defineOptions({ name: 'ChatAgent' })

  const { containerMinHeight } = useAutoLayoutHeight()

  const userStore = useUserStore()
  const agentStore = useAgentStore()
  const chatThreadsStore = useChatThreadsStore()
  const route = useRoute()
  const router = useRouter()

  // 输入框纯文本（与 MessageInputComponent 双向同步）
  const inputText = ref('')
  const searchQuery = ref('')
  const sending = ref(false)
  const elementsConversationRef = ref(null)
  const messageInputRef = ref(null)
  const sidebarCollapsed = ref(false)
  const isLoadingMessages = ref(false)

  // 附件系统状态
  const attachmentUploadModalOpen = ref(false)
  const threadAttachmentsMap = ref({})
  // 本轮待发送附件（仅本次消息携带的附件集合）。成功发送后清空；失败则回滚。
  // 与 threadAttachmentsMap 的关系：
  //   - threadAttachmentsMap：thread 的全部附件（不会随发送清空，会复用）。
  //   - pendingAttachmentIdsByThread：仅含尚未绑定 request_id 的"新附件"
  //     （被本轮请求消费、待 markAttachmentsRequestId 后清空）。
  //   - 此分离避免把已发送过的历史附件反复随每条消息提交。
  const pendingAttachmentIdsByThread = ref({})
  const getPendingSetForThread = (tid) => {
    if (!tid) return new Set()
    if (!pendingAttachmentIdsByThread.value[tid]) {
      pendingAttachmentIdsByThread.value[tid] = new Set()
    }
    return pendingAttachmentIdsByThread.value[tid]
  }
  const currentThreadAttachments = computed(() => {
    const tid = currentThreadId.value
    return tid ? threadAttachmentsMap.value[tid] || [] : []
  })

  // 附件 -> FilesCardProps 适配（供 Attachments 组件使用）
  const inferFileType = (fileName) => {
    const ext = (fileName || '').split('.').pop()?.toLowerCase() || ''
    const map = {
      pdf: 'pdf',
      doc: 'word',
      docx: 'word',
      xls: 'excel',
      xlsx: 'excel',
      ppt: 'ppt',
      pptx: 'ppt',
      txt: 'txt',
      md: 'mark',
      png: 'image',
      jpg: 'image',
      jpeg: 'image',
      gif: 'image',
      webp: 'image',
      svg: 'image',
      mp3: 'audio',
      wav: 'audio',
      mp4: 'video',
      avi: 'video',
      mov: 'video',
      zip: 'zip',
      rar: 'zip',
      '7z': 'zip',
      json: 'code',
      js: 'code',
      ts: 'code',
      py: 'code',
      java: 'code',
      go: 'code',
      rs: 'code',
      c: 'code',
      cpp: 'code',
      html: 'code',
      css: 'code',
      sql: 'database'
    }
    return map[ext] || 'file'
  }
  const attachmentCardItems = computed(() => {
    return currentThreadAttachments.value.map((att) => ({
      uid: att.file_id,
      name: att.file_name || '未命名文件',
      fileType: inferFileType(att.file_name),
      status: 'done',
      showDelIcon: true,
      description: att.parsed_object_name ? '已解析' : ''
    }))
  })

  // 文件工作区 / 状态面板
  const fileWorkspaceOpen = ref(false)
  const statePanelOpen = ref(false)

  // ==================== 会话列表 ====================
  const sortMode = ref('time') // time | name

  const filteredThreads = computed(() => {
    let list = [...chatThreadsStore.threads]
    // 搜索过滤
    if (searchQuery.value.trim()) {
      const q = searchQuery.value.toLowerCase().trim()
      list = list.filter((t) => (t.title || '新的对话').toLowerCase().includes(q))
    }
    // 排序：置顶优先 + 时间倒序 / 名称
    list.sort((a, b) => {
      if (a.is_pinned && !b.is_pinned) return -1
      if (!a.is_pinned && b.is_pinned) return 1
      if (sortMode.value === 'name') {
        return (a.title || '').localeCompare(b.title || '', 'zh-CN')
      }
      return new Date(b.updated_at || 0) - new Date(a.updated_at || 0)
    })
    return list
  })

  const onThreadListScroll = ({ scrollTop, scrollHeight, clientHeight }) => {
    if (chatThreadsStore.isLoadingMoreThreads || !chatThreadsStore.hasMoreThreads) return
    if (scrollHeight - scrollTop - clientHeight < 50) {
      chatThreadsStore.loadMoreThreads()
    }
  }

  // 随机打招呼语
  const greetingText = '描述设备现象、故障码或维修问题，我会结合知识库给出可核验的排查建议。'

  // 空状态建议问题
  const starterPrompts = [
    { key: 'fault-code', label: '查询设备故障码与处置建议' },
    { key: 'diagnosis', label: '根据故障现象生成排查步骤' },
    { key: 'case', label: '查找相似的历史维修案例' },
    { key: 'review', label: '汇总维修记录并形成复盘' }
  ]
  const handlePromptClick = (item) => {
    if (item?.label) {
      messageInputRef.value?.focus?.()
      // MessageInputComponent 使用 v-model:text，直接设 inputText 即可
      inputText.value = item.label
    }
  }

  // MessageInputComponent @update:modelValue：同步文本
  const handleInputUpdate = (value) => {
    inputText.value = value
  }

  // MessageInputComponent @keydown：处理 Enter 发送
  const handleInputKeyDown = (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      handleSendOrStop()
    }
  }

  // 提及配置：从 configurableItems + agentConfig 提取知识库/MCP/Skill/Subagent（对齐 Yuxi useAgentMentionConfig）
  const mentionConfig = computed(() => {
    // files: 当前线程已上传的附件
    const files = currentThreadAttachments.value
      .filter((att) => att.file_name)
      .map((att) => ({
        id: att.file_id,
        name: att.file_name,
        path: att.path || att.file_name
      }))

    // 从 configurableItems 提取资源类候选
    const configItems = agentStore.configurableItems || []
    const currentConfig = agentStore.agentConfig || {}
    const includeAllByKind = { knowledges: false, mcps: false, skills: false, subagents: false }
    const selectedByKind = {
      knowledges: new Set(),
      mcps: new Set(),
      skills: new Set(),
      subagents: new Set()
    }
    const optionsByKind = {
      knowledges: new Map(),
      mcps: new Map(),
      skills: new Map(),
      subagents: new Map()
    }
    const resourceItems = []

    // configurableItems 可能是数组或对象，统一处理
    const items = Array.isArray(configItems) ? configItems : Object.values(configItems)
    items.forEach((item) => {
      const kind = item?.kind
      if (!isMentionAgentResourceKind(kind)) return
      resourceItems.push({ kind, item })
      const val = currentConfig[item.key || item.name]
      // val === null 或 undefined（未配置）时，默认-all 资源类按"包含全部"处理
      if ((val === null || val === undefined) && isDefaultAllAgentResourceKind(kind)) {
        includeAllByKind[kind] = true
      } else if (Array.isArray(val)) {
        val.forEach((value) => selectedByKind[kind].add(value))
      }
    })

    resourceItems.forEach(({ kind, item }) => {
      const selectedValues = selectedByKind[kind]
      if (!includeAllByKind[kind] && !selectedValues.size) return
      getAgentConfigOptions(item).forEach((option) => {
        const value = getAgentConfigOptionValue(option)
        if (!value || (!includeAllByKind[kind] && !selectedValues.has(value))) return
        const name = getAgentConfigOptionLabel(option) || value
        const description = getAgentConfigOptionDescription(option)
        let normalized
        if (kind === 'knowledges') {
          normalized = { kb_id: value, name, description }
        } else if (kind === 'subagents') {
          normalized = { id: value, slug: option?.slug || value, name, description }
        } else {
          normalized = { slug: value, name, description }
        }
        if (normalized) optionsByKind[kind].set(value, normalized)
      })
    })

    const selectOptions = (kind) => {
      const result = []
      const optionMap = optionsByKind[kind]
      if (includeAllByKind[kind]) {
        optionMap.forEach((option) => result.push(option))
        return result
      }
      selectedByKind[kind].forEach((value) => {
        const option = optionMap.get(value)
        if (option) result.push(option)
      })
      return result
    }

    const result = {
      files,
      knowledgeBases: selectOptions('knowledges'),
      mcps: selectOptions('mcps'),
      skills: selectOptions('skills'),
      subagents: selectOptions('subagents')
    }
    return result
  })

  // 重新生成
  const retryMessage = async (bubbleItem) => {
    if (isStreaming.value || sending.value) {
      ElMessage.warning('请等待当前回复完成')
      return
    }
    const retrySource = resolveRetryUserMessage({
      target: bubbleItem,
      conversations: displayConversations.value,
      bubbleItems: bubbleListItems.value
    })
    const userContent = retrySource?.content || ''
    if (!userContent) {
      ElMessage.warning('未找到原始问题')
      return
    }

    const agentId = agentStore.selectedAgentId
    const threadId = currentThreadId.value
    if (!agentId || !threadId) return

    sending.value = true
    let retryRequestId = ''
    let retryThreadState = null
    try {
      const ts = getThreadState(threadId)
      const requestId = `retry-${Date.now()}`
      retryRequestId = requestId
      retryThreadState = ts
      ts.pendingRequestId = requestId
      ts.onGoingConv.msgChunks[requestId] = [
        {
          id: requestId,
          type: 'human',
          content: userContent,
          extra_metadata: { request_id: requestId }
        }
      ]
      onScrollToBottom()

      const runResp = await agentApi.createAgentRun({
        query: userContent,
        agent_id: agentId,
        thread_id: threadId,
        meta: { request_id: requestId }
      })
      const runId = runResp?.run_id
      if (!runId) throw new Error('创建任务失败：缺少 run_id')
      await startRunStream(threadId, runId, '0-0')
    } catch (e) {
      console.error('retry error', e)
      if (retryThreadState && retryRequestId) {
        delete retryThreadState.onGoingConv.msgChunks[retryRequestId]
        if (retryThreadState.pendingRequestId === retryRequestId) {
          retryThreadState.pendingRequestId = null
        }
      }
      ElMessage.error(e?.message || '重新生成失败')
    } finally {
      sending.value = false
    }
  }

  // 附件系统
  const openAttachmentModal = () => {
    attachmentUploadModalOpen.value = true
  }
  const ensureAttachmentThread = async () => {
    if (currentThreadId.value) return currentThreadId.value
    let agentId = agentStore.selectedAgentId
    if (!agentId) {
      await agentStore.initialize()
      agentId = agentStore.selectedAgentId
    }
    if (!agentId) return ''
    const thread = await chatThreadsStore.createThread(agentId, '新的对话')
    if (thread?.id) {
      chatThreadsStore.setCurrentThreadId(thread.id)
      return thread.id
    }
    return ''
  }
  const fetchThreadAttachments = async (threadId) => {
    if (!threadId) return
    try {
      const response = await threadApi.getThreadAttachments(threadId)
      const attachments = Array.isArray(response?.attachments) ? response.attachments : []
      threadAttachmentsMap.value[threadId] = attachments
      // 标记"未绑定 request_id"的附件为待发送。本轮请求消费这些 file_id 后，
      // 后端的 _bind_request_attachments 会把它们写上 request_id；
      // 下一次 fetch 看到 request_id 就不会再加回 pending。
      const pendingSet = getPendingSetForThread(threadId)
      for (const att of attachments) {
        if (att?.file_id && !att.request_id) pendingSet.add(att.file_id)
      }
    } catch (e) {
      console.warn('Failed to fetch thread attachments:', e)
      threadAttachmentsMap.value[threadId] = []
    }
  }
  const handleTmpAttachmentsAdded = async () => {
    const tid = currentThreadId.value
    if (!tid) return
    await fetchThreadAttachments(tid)
  }
  const handleAttachmentRemove = async (attachment) => {
    const tid = currentThreadId.value
    const fileId = attachment?.file_id
    if (!tid || !fileId) return
    const prev = threadAttachmentsMap.value[tid] || []
    threadAttachmentsMap.value[tid] = prev.filter((a) => a.file_id !== fileId)
    try {
      await threadApi.deleteThreadAttachment(tid, fileId)
    } catch (e) {
      threadAttachmentsMap.value[tid] = prev
      ElMessage.error(e?.message || '删除附件失败')
    }
  }

  // 智能体下拉命令
  const agentEditModalRef = ref(null)
  const handleAgentCommand = (cmd) => {
    if (cmd.action === 'select') {
      if (
        currentThreadId.value &&
        currentThreadAgentId.value &&
        cmd.agentId !== currentThreadAgentId.value
      ) {
        ElMessage.info('当前对话已绑定智能体，请新建对话后切换')
        return
      }
      agentStore.selectAgent(cmd.agentId)
    } else if (cmd.action === 'manage') {
      const id = agentStore.selectedAgentId
      if (id) agentEditModalRef.value?.openEdit(id)
    }
  }

  // 用户信息
  const userAvatar = computed(() => userStore.info?.avatar || '')
  const userAvatarSrc = computed(() => usableMediaSrc(userAvatar.value) || defaultUserAvatar)
  const userName = computed(() => userStore.info?.userName || '用户')
  const userEmail = computed(() => userStore.info?.email || '')
  const getAgentAvatarSrc = (agent) => {
    const explicitAvatar = usableMediaSrc(
      agent?.icon || agent?.avatar || agent?.avatar_url || agent?.metadata?.icon
    )
    if (explicitAvatar) return explicitAvatar
    const seed = agent?.id || agent?.agent_id || agent?.slug || agent?.name || 'default-chatbot'
    return generatePixelAvatar(seed)
  }
  const agentAvatarSrc = computed(() => getAgentAvatarSrc(agentStore.selectedAgent))
  const threadAvatarStyleCache = new Map()
  const getThreadAvatarStyle = (thread) => {
    const key = String(thread?.id || thread?.thread_id || thread?.title || 'thread-avatar')
    if (!threadAvatarStyleCache.has(key)) {
      threadAvatarStyleCache.set(key, createThreadGradientStyle(key))
    }
    return threadAvatarStyleCache.get(key)
  }

  const chatState = reactive({ threadStates: {} })
  const historyMessages = reactive({})

  const currentThreadId = computed(() => chatThreadsStore.currentThreadId)
  const currentThread = computed(() => chatThreadsStore.currentThread)
  const currentThreadAgentId = computed(() => currentThread.value?.agent_id || '')
  const currentAgentName = computed(
    () => agentStore.selectedAgent?.name || agentStore.selectedAgentId || '智能体'
  )

  // 对话级模型选择
  const DRAFT_MODEL_KEY = '__draft__'
  const selectedModelByThread = reactive({})
  const agentDefaultModel = computed(() => {
    // agentConfig.model 可能是空字符串（后端返回 ""），需 fallback 到 configurableItems 的 default
    const fromConfig = agentStore.agentConfig?.model
    if (fromConfig) return fromConfig
    const modelItem = agentStore.configurableItems.find(
      (i) => i.key === 'model' || i.kind === 'llm'
    )
    return modelItem?.default || agentStore.selectedAgent?.config_context?.model || ''
  })
  const currentModelSpec = computed(
    () =>
      selectedModelByThread[currentThreadId.value || DRAFT_MODEL_KEY] ||
      agentDefaultModel.value ||
      ''
  )
  const handleModelSelect = (spec) => {
    if (typeof spec === 'string' && spec) {
      selectedModelByThread[currentThreadId.value || DRAFT_MODEL_KEY] = spec
    }
  }
  const restoreThreadModelSelection = (threadId, history) => {
    if (selectedModelByThread[threadId]) return
    for (let i = history.length - 1; i >= 0; i -= 1) {
      const msg = history[i]
      if (msg?.type !== 'human') continue
      const modelSpec = msg?.extra_metadata?.model_spec
      if (modelSpec) {
        selectedModelByThread[threadId] = modelSpec
        return
      }
    }
  }

  const { getThreadState, resetOnGoingConv } = useAgentThreadState({
    chatState,
    getCurrentThreadId: () => currentThreadId.value
  })

  const streamSmoother = useStreamSmoother({ getThreadState })

  const {
    approvalState,
    processApprovalInStream,
    restoreInterruptFromThreadState,
    hideApprovalState
  } = useApproval({
    getThreadState,
    fetchThreadMessages: ({ threadId }) => fetchThreadMessages({ threadId })
  })

  const { handleStreamChunk } = useAgentStreamHandler({
    getThreadState,
    processApprovalInStream,
    currentAgentId: computed(() => agentStore.selectedAgentId),
    streamSmoother
  })

  const fetchThreadMessages = async ({ threadId, delay = 0 }) => {
    if (!threadId) return
    if (delay > 0) {
      await new Promise((r) => setTimeout(r, delay))
    }
    isLoadingMessages.value = true
    try {
      const response = await agentApi.getAgentHistory(threadId)
      historyMessages[threadId] = response.history || []
      restoreThreadModelSelection(threadId, response.history || [])
      void fetchThreadAttachments(threadId)
      onScrollToBottom()
    } catch (e) {
      console.error('fetchThreadMessages error', e)
    } finally {
      isLoadingMessages.value = false
    }
  }

  // AgentState
  const stateLoadError = ref('')
  const normalizeAgentState = (response) => {
    const payload =
      response?.agent_state ?? response?.data?.agent_state ?? response?.data ?? response
    if (!payload || typeof payload !== 'object' || Array.isArray(payload)) return null
    return {
      todos: [],
      files: {},
      artifacts: [],
      subagent_runs: [],
      ...payload
    }
  }

  const fetchAgentState = async (threadId = null) => {
    const tid = threadId || currentThreadId.value
    if (!tid) return
    stateLoadError.value = ''
    try {
      const res = await agentApi.getAgentState(tid)
      const targetState = getThreadState(tid)
      if (!targetState) return
      targetState.agentState = normalizeAgentState(res)
    } catch (error) {
      stateLoadError.value = error?.message || '状态加载失败'
    }
  }

  const isRefreshingState = ref(false)
  const handleAgentStateRefresh = async () => {
    const tid = currentThreadId.value
    if (!tid) return
    isRefreshingState.value = true
    try {
      await fetchAgentState(tid)
    } finally {
      isRefreshingState.value = false
    }
  }

  const filePanelWidth = 420

  const toggleStatePanel = async () => {
    const nextOpen = !statePanelOpen.value
    statePanelOpen.value = nextOpen
    if (nextOpen && currentThreadId.value && !currentAgentState.value) {
      await handleAgentStateRefresh()
    }
  }

  const toggleFileWorkspace = async () => {
    const nextOpen = !fileWorkspaceOpen.value
    fileWorkspaceOpen.value = nextOpen
    if (nextOpen && currentThreadId.value && !currentAgentState.value) {
      await handleAgentStateRefresh()
    }
  }

  const onScrollToBottom = () => {
    nextTick(() => {
      elementsConversationRef.value?.scrollToBottom?.(false)
    })
  }

  const { startRunStream, resumeActiveRunForThread, stopRunStreamSubscription } = useAgentRunStream(
    {
      getThreadState,
      currentAgentId: computed(() => agentStore.selectedAgentId),
      handleStreamChunk,
      fetchThreadMessages,
      fetchAgentState,
      resetOnGoingConv,
      onScrollToBottom,
      streamSmoother,
      onInterruptDetected: ({ threadId }) => {
        restoreInterruptFromThreadState(threadId)
      }
    }
  )

  // 时间格式化
  const formatChatTime = (isoStr) => {
    if (!isoStr) return ''
    const normalized = /[zZ]$|[+-]\d{2}:\d{2}$/.test(isoStr) ? isoStr : isoStr + 'Z'
    const d = new Date(normalized)
    if (isNaN(d.getTime())) return ''
    return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
  }

  function extractSourcesFromMessage(msg) {
    if (!msg || msg.type !== 'ai') return { knowledgeChunks: [], webSources: [] }
    return MessageProcessor.extractSourcesFromMessage(msg, [])
  }

  function toDisplayMessage(msg, prefix, idx) {
    const isAi = msg.type === 'ai' || msg.type === 'AIMessageChunk'
    const isSystem = msg.type === 'system' || msg.role === 'system'
    const body = isAi
      ? MessageProcessor.parseAssistantMessageBody(msg)
      : { content: msg.content, reasoningContent: '' }
    const errorType = msg.extra_metadata?.error_type || msg.error_type || ''
    const errorMessage =
      msg.extra_metadata?.error_message ||
      msg.error_message ||
      {
        interrupted: '回答生成已中断',
        content_guard_blocked: '检测到敏感内容，已中断输出',
        unexpect: '生成过程中出现异常',
        agent_error: '智能体获取失败',
        content_filter: '内容被安全策略拦截'
      }[errorType] ||
      ''
    return {
      key: `${prefix}-${msg.id || idx}`,
      role: isSystem ? 'system' : msg.type === 'human' ? 'user' : 'assistant',
      content: typeof body.content === 'string' ? body.content : '',
      avatar: msg.type === 'human' ? userAvatarSrc.value : agentAvatarSrc.value,
      time: msg.created_at ? formatChatTime(msg.created_at) : '',
      sender: msg.type === 'human' ? userName.value : currentAgentName.value,
      id: msg.id,
      feedback: msg.feedback,
      reasoning: body.reasoningContent || '',
      modelName: msg.response_metadata?.model_name || msg.extra_metadata?.model_spec?.model || '',
      toolCalls: isAi ? msg.tool_calls || [] : [],
      sources: isAi ? extractSourcesFromMessage(msg) : { knowledgeChunks: [], webSources: [] },
      errorType,
      errorMessage,
      isStoppedByUser: msg.isStoppedByUser || msg.extra_metadata?.is_stopped_by_user || false,
      userAttachments: msg.type === 'human' ? msg.extra_metadata?.attachments || [] : [],
      imageContent: msg.type === 'human' ? msg.image_content || '' : '',
      imageMime:
        msg.type === 'human'
          ? msg.extra_metadata?.image_mime || msg.extra_metadata?.mime_type || 'image/jpeg'
          : 'image/jpeg',
      raw: msg
    }
  }

  // 显示对话列表 = 历史(按轮次分组) + 当前流式
  const displayConversations = computed(() => {
    const threadId = currentThreadId.value
    if (!threadId) return []
    const ts = getThreadState(threadId)

    const history = historyMessages[threadId] || []
    const mergedHistory = MessageProcessor.convertToolResultToMessages(history)
    const conversations = MessageProcessor.convertServerHistoryToMessages(mergedHistory)

    if (ts) {
      const msgChunks = ts.onGoingConv.msgChunks
      const ongoingMessages = []
      Object.keys(msgChunks).forEach((key) => {
        const chunks = msgChunks[key]
        if (chunks?.length) {
          const merged = MessageProcessor.mergeMessageChunk(chunks)
          if (merged && (merged.content || merged.type === 'human' || merged.tool_calls?.length)) {
            ongoingMessages.push(merged)
          }
        }
      })

      if (ongoingMessages.length > 0) {
        const mergedOngoing = MessageProcessor.convertToolResultToMessages(ongoingMessages)
        // 过滤掉 type='tool' 的消息（工具结果已通过 convertToolResultToMessages 回填到 AI 消息的 tool_calls）
        const filteredOngoing = mergedOngoing.filter((m) => m.type !== 'tool')
        conversations.push({
          messages: filteredOngoing,
          status: 'loading',
          isOngoing: true
        })
      }
    }

    return conversations
  })

  // 为每轮对话生成 display items
  const toDisplayAssistantTurn = (item, prefix) => {
    const representative = toDisplayMessage(item.message, prefix, item.sourceIndex)
    const messageDisplays = item.messages.map((message, index) =>
      toDisplayMessage(message, prefix, item.sourceIndex + index)
    )
    const latestError = [...messageDisplays]
      .reverse()
      .find((display) => display.errorType || display.isStoppedByUser)
    const parts = []

    item.parts.forEach((part, partIndex) => {
      if (part.type === 'reasoning') {
        parts.push({ type: 'reasoning', payload: { reasoning: part.content } })
      } else if (part.type === 'text') {
        parts.push({ type: 'text', payload: { content: part.content } })
      } else if (part.type === 'tool-group') {
        parts.push({ type: 'tool-group', payload: { toolCalls: part.toolCalls } })
      }

      const nextPart = item.parts[partIndex + 1]
      if (nextPart?.message === part.message) return
      const sources = extractSourcesFromMessage(part.message)
      if (sources.knowledgeChunks.length || sources.webSources.length) {
        parts.push({ type: 'sources', payload: { sources } })
      }
    })

    return {
      ...representative,
      key: item.key,
      role: 'assistant',
      content: parts
        .filter((part) => part.type === 'text')
        .map((part) => part.payload.content)
        .join('\n\n'),
      reasoning: parts
        .filter((part) => part.type === 'reasoning')
        .map((part) => part.payload.reasoning)
        .join('\n\n'),
      parts,
      errorType: latestError?.errorType || '',
      errorMessage: latestError?.errorMessage || '',
      isStoppedByUser: latestError?.isStoppedByUser || false
    }
  }

  const displayItems = computed(() => {
    const convs = displayConversations.value
    const result = []
    convs.forEach((conv, convIdx) => {
      const items = getConversationDisplayItems(conv)
      items.forEach((item) => {
        const prefix = conv.isOngoing ? 'ongoing' : 'history'
        if (item.type === 'message') {
          const msg = item.message
          result.push({
            ...item,
            convIdx,
            display: toDisplayMessage(msg, prefix, item.sourceIndex)
          })
        } else if (item.type === 'assistant-turn') {
          result.push({
            ...item,
            convIdx,
            display: toDisplayAssistantTurn(item, prefix)
          })
        }
      })
    })
    return result
  })

  const displayMessages = computed(() => {
    return displayItems.value.filter((item) => item.type === 'message').map((item) => item.display)
  })

  // ==================== 消息列表数据适配 ====================
  // 将 displayItems 转换为会话消息结构
  const bubbleListItems = computed(() => {
    const convs = displayConversations.value
    const lastConvIdx = convs.length - 1
    const isLastConvStreaming = lastConvIdx >= 0 && convs[lastConvIdx]?.isOngoing
    return displayItems.value.map((item) => {
      const d = item.display
      const isUser = d.role === 'user'
      // 产物卡片仅展示在最后一轮对话的助手消息上，且当前不在流式中
      const showArtifacts =
        d.role === 'assistant' &&
        item.convIdx === lastConvIdx &&
        !isLastConvStreaming &&
        !isStreaming.value &&
        currentArtifacts.value.length > 0
      return {
        key: d.key,
        content: d.content || '',
        placement: isUser ? 'end' : 'start',
        variant: isUser ? 'outlined' : 'filled',
        shape: 'corner',
        maxWidth: '70%',
        loading: false,
        // 自定义字段（在 #content / #header / #footer slot 中使用）
        _type: 'message',
        _avatar: usableMediaSrc(d.avatar),
        _sender: d.sender,
        _time: d.time,
        _reasoning: d.reasoning,
        _parts: isUser ? undefined : d.parts,
        _errorType: d.errorType,
        _errorMessage: d.errorMessage,
        _isStoppedByUser: d.isStoppedByUser,
        _sources: d.sources,
        _artifacts: showArtifacts ? currentArtifacts.value : [],
        _msgId: d.id,
        _feedback: d.feedback,
        _modelName: d.modelName,
        _userAttachments: d.userAttachments || [],
        _imageContent: d.imageContent || '',
        _imageMime: d.imageMime || 'image/jpeg',
        _convIdx: item.convIdx
      }
    })
  })

  const isReplyLoading = computed(() => {
    const threadId = currentThreadId.value
    if (!threadId) return false
    return getThreadState(threadId)?.replyLoadingVisible || false
  })

  const isStreaming = computed(() => {
    const threadId = currentThreadId.value
    if (!threadId) return false
    return getThreadState(threadId)?.isStreaming || false
  })

  const isConversationEmpty = computed(
    () =>
      bubbleListItems.value.length === 0 &&
      currentThreadAttachments.value.length === 0 &&
      !isLoadingMessages.value &&
      !isReplyLoading.value
  )

  const currentAgentState = computed(() => {
    const threadId = currentThreadId.value
    if (!threadId) return null
    return getThreadState(threadId)?.agentState || null
  })

  const currentArtifacts = computed(() => {
    const artifacts = currentAgentState.value?.artifacts
    return Array.isArray(artifacts) ? artifacts : []
  })

  // 状态面板汇总
  const stateTodos = computed(() =>
    Array.isArray(currentAgentState.value?.todos) ? currentAgentState.value.todos : []
  )
  const stateFiles = computed(() => {
    const raw = currentAgentState.value?.files
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
      return Object.keys(raw).length
    }
    return 0
  })
  const stateSubagentRuns = computed(() =>
    Array.isArray(currentAgentState.value?.subagent_runs)
      ? currentAgentState.value.subagent_runs.length
      : 0
  )
  const hasTokenUsage = computed(() => {
    const u = currentAgentState.value?.token_usage
    return Boolean(u && typeof u === 'object')
  })
  const hasVisibleStateSections = computed(
    () =>
      hasTokenUsage.value ||
      stateTodos.value.length > 0 ||
      stateFiles.value > 0 ||
      currentArtifacts.value.length > 0 ||
      stateSubagentRuns.value > 0
  )
  const stateSummaryLabel = computed(() => {
    if (!hasVisibleStateSections.value) return '暂无内容'
    const total =
      (hasTokenUsage.value ? 1 : 0) +
      stateTodos.value.length +
      stateFiles.value +
      currentArtifacts.value.length +
      stateSubagentRuns.value
    return `${total} 项`
  })

  // 发送消息
  const send = async () => {
    // MessageInputComponent 的文本已含标准 mention token（@file:xxx），直接使用
    const text = String(inputText.value || '')
      .replace(/^\n+/, '')
      .replace(/\n+$/, '')
      .trim()
    if (!text || sending.value) return

    // 从 mention token 中提取 file 类的 attachment_file_ids
    const triggerFileIds = new Set()
    const segments = parseMentionText(text)
    for (const seg of segments) {
      if (seg.kind === 'mention' && seg.type === 'file') {
        triggerFileIds.add(seg.value)
      }
    }

    let agentId = agentStore.selectedAgentId
    if (!agentId) {
      await agentStore.initialize()
      agentId = agentStore.selectedAgentId
    }
    if (!agentId) {
      ElMessage.error('未选中智能体')
      return
    }

    sending.value = true
    // 提升到 try 外，避免 catch 块处于 TDZ。
    let threadId = currentThreadId.value
    let pendingSnapshot = []
    try {
      if (!threadId) {
        const thread = await chatThreadsStore.createThread(agentId, '新的对话')
        threadId = thread?.id
        if (threadId) {
          chatThreadsStore.setCurrentThreadId(threadId)
          // 同步 URL：新建对话默认聚焦在此 thread_id 上
          await syncRouteThreadId(threadId, { replace: true })
        }
      }
      if (!threadId) {
        ElMessage.error('创建对话失败')
        return
      }

      // 首条消息自动生成标题
      if ((historyMessages[threadId] || []).length === 0) {
        const autoTitle = text.replace(/\s+/g, ' ').trim().slice(0, 2000)
        if (autoTitle) {
          void (async () => {
            try {
              const generatedTitle = await agentApi.generateTitle(autoTitle, null)
              if (generatedTitle) {
                const finalTitle = generatedTitle.slice(0, 30).replace(/\s+/g, ' ').trim()
                if (finalTitle) {
                  void chatThreadsStore.updateThread(threadId, finalTitle).catch(() => {})
                }
              }
            } catch (e) {
              console.error('Title generation failed:', e)
              void chatThreadsStore.updateThread(threadId, autoTitle.slice(0, 30)).catch(() => {})
            }
          })()
        }
      }

      const ts = getThreadState(threadId)
      const requestId = `local-${Date.now()}`
      ts.pendingRequestId = requestId
      ts.onGoingConv.msgChunks[requestId] = [{ id: requestId, type: 'human', content: text }]

      inputText.value = ''
      // MessageInputComponent 通过 watch(inputValue) 自动同步清空
      onScrollToBottom()

      const draftModelSpec = selectedModelByThread[DRAFT_MODEL_KEY]
      if (draftModelSpec && !selectedModelByThread[threadId]) {
        selectedModelByThread[threadId] = draftModelSpec
        delete selectedModelByThread[DRAFT_MODEL_KEY]
      }
      const modelSpec = selectedModelByThread[threadId] || null

      // 本轮消费 pending 附件：从 pending 集合取出并立刻移除（移交给后端）。
      // 成功后端会把 request_id 写到每条记录上，下次 fetch 时这些 file_id 不再进入 pending；
      // 失败则在 catch 中恢复。
      const pendingSet = getPendingSetForThread(threadId)
      pendingSnapshot = Array.from(pendingSet)
      for (const fid of pendingSnapshot) pendingSet.delete(fid)
      // 合并：用户通过 @ 触发的 file_id 也作为本轮附件。
      const combinedAttachmentFileIds = Array.from(
        new Set([...pendingSnapshot, ...triggerFileIds])
      ).filter(Boolean)

      const runResp = await agentApi.createAgentRun({
        query: text,
        agent_id: agentId,
        thread_id: threadId,
        meta: { request_id: requestId, attachment_file_ids: combinedAttachmentFileIds },
        image_content: null,
        model_spec: modelSpec
      })
      const runId = runResp?.run_id
      if (!runId) {
        // 失败回滚：把快照中的 file_id 还给 pending 集合
        for (const fid of pendingSnapshot) pendingSet.add(fid)
        throw new Error('创建任务失败：缺少 run_id')
      }

      await startRunStream(threadId, runId, '0-0')
    } catch (e) {
      console.error('send error', e)
      // 兜底回滚：异常路径
      try {
        if (threadId) {
          const pendingSet = pendingAttachmentIdsByThread.value[threadId]
          if (pendingSet && typeof pendingSnapshot !== 'undefined' && pendingSnapshot.length) {
            const restoreIds = new Set(pendingSnapshot)
            // 仅回滚尚未存在的（避免覆盖后续并发 upload）
            for (const fid of restoreIds) {
              if (!pendingSet.has(fid)) pendingSet.add(fid)
            }
          }
        }
      } catch {
        // 保留原始发送错误，由下方统一提示用户。
      }
      ElMessage.error(e?.message || '发送失败')
    } finally {
      sending.value = false
    }
  }

  // 发送 / 停止双态
  const handleSendOrStop = async () => {
    const threadId = currentThreadId.value
    const ts = getThreadState(threadId)
    if (isStreaming.value && ts?.activeRunId) {
      try {
        await agentApi.cancelAgentRun(ts.activeRunId)
        const msgChunks = ts.onGoingConv.msgChunks
        Object.keys(msgChunks).forEach((key) => {
          const chunks = msgChunks[key]
          if (chunks?.length) {
            const merged = MessageProcessor.mergeMessageChunk(chunks)
            if (merged && (merged.type === 'ai' || merged.type === 'AIMessageChunk')) {
              merged.isStoppedByUser = true
              msgChunks[key] = [merged]
            }
          }
        })
        ElMessage.info('已发送取消请求')
      } catch (e) {
        ElMessage.error(e?.message || '停止失败')
      }
      return
    }
    await send()
  }

  // ==================== 人工审批处理 ====================
  const handleApprovalWithStream = async (answer) => {
    const threadId = approvalState.threadId
    const parentRunId = approvalState.parentRunId
    if (!threadId) {
      ElMessage.error('无效的提问请求')
      approvalState.showModal = false
      return
    }

    const threadState = getThreadState(threadId)
    if (!threadState) {
      ElMessage.error('无法找到对应的对话线程')
      approvalState.showModal = false
      return
    }

    if (!parentRunId) {
      ElMessage.error('无法找到需要恢复的运行任务')
      approvalState.showModal = false
      return
    }

    const pendingInterrupt = threadState.pendingInterrupt

    try {
      hideApprovalState()
      threadState.pendingInterrupt = null
      threadState.isStreaming = true
      resetOnGoingConv(threadId)
      const resumeRequestId = `resume-${Date.now()}`
      const runResp = await agentApi.createAgentRun({
        query: null,
        agent_id: agentStore.selectedAgentId,
        thread_id: threadId,
        meta: { request_id: resumeRequestId },
        resume: answer,
        parent_run_id: parentRunId,
        resume_request_id: resumeRequestId
      })
      const runId = runResp?.run_id
      if (!runId) throw new Error('创建 resume run 失败：缺少 run_id')
      await startRunStream(threadId, runId, '0-0')
    } catch (error) {
      if (pendingInterrupt) {
        threadState.pendingInterrupt = pendingInterrupt
        restoreInterruptFromThreadState(threadId)
      }
      threadState.isStreaming = false
      threadState.replyLoadingVisible = false
      ElMessage.error(error?.message || '恢复运行失败')
    }
  }

  const handleApprovalSubmit = (answer) => {
    handleApprovalWithStream(answer)
  }

  const handleApprovalCancel = () => {
    handleApprovalWithStream('reject')
  }

  // 图片放大预览
  const imagePreviewVisible = ref(false)
  const imagePreviewSrc = ref('')
  const openImagePreview = (mime, base64) => {
    if (!base64) return
    imagePreviewSrc.value = `data:${mime || 'image/jpeg'};base64,${base64}`
    imagePreviewVisible.value = true
  }

  // 复制消息
  const copyMessage = async (content) => {
    try {
      await navigator.clipboard.writeText(content || '')
      ElMessage.success('已复制')
    } catch {
      ElMessage.error('复制失败')
    }
  }

  // 能力判断（来自 selectedAgent.capabilities）
  // Yuxi 实现：return capabilities.includes('file_upload') 等；
  //   若该 agent 尚未拉取 capabilities 字段则默认 false（保守）。
  //   我们加一层兜底：如果 selectedAgent 没有任何 capabilities 字段，
  //   回退到行为开启（保持历史兼容性），让用户能继续上传。
  const selectedAgentCapabilities = computed(() => {
    const caps = agentStore.selectedAgent?.capabilities
    return Array.isArray(caps) ? caps : null
  })
  const supportsFileUpload = computed(() => {
    const caps = selectedAgentCapabilities.value
    if (caps === null) return true // capabilities 字段缺失 -> 默认开启
    return caps.includes('file_upload')
  })
  const supportsFiles = computed(() => {
    const caps = selectedAgentCapabilities.value
    if (caps === null) return true
    return caps.includes('files')
  })

  // ============== P0-3: 统一会话选择 + 路由同步 ==============

  // 把 URL 上的 thread_id 同步到当前路由；tid 为空时移除 query 字段
  const syncingFromRoute = ref(false)
  const isChatRoute = () => route.name === 'Chat' || route.path === '/chat'
  const syncRouteThreadId = (threadId) => {
    // keepAlive 下组件会留存：离开对话页后禁止仅 replace query，以免干扰其它页面导航
    if (!isChatRoute()) return
    const desired = threadId || ''
    const current = String(route.query?.thread_id || '')
    if (desired === current) return
    const nextQuery = { ...route.query }
    if (desired) {
      nextQuery.thread_id = desired
    } else {
      delete nextQuery.thread_id
    }
    router.replace({ name: 'Chat', query: nextQuery })
  }

  /**
   * 统一会话切换入口
   * @param {string|null} threadId - 目标 thread id；空或 null 表示清除当前选中
   * @param {object} opts
   * @param {boolean} opts.fromUrl - 当 URL 反向驱动切换时为 true，避免再回写 URL
   * @returns {Promise<boolean>} 是否成功
   */
  const selectThread = async (threadId, { fromUrl = false } = {}) => {
    if (!threadId) {
      if (currentThreadId.value) {
        stopRunStreamSubscription(currentThreadId.value)
      }
      chatThreadsStore.setCurrentThreadId(null)
      if (!fromUrl) syncRouteThreadId(null)
      return true
    }

    // 已经是当前 thread：可能仍要把 URL 拉齐
    if (currentThreadId.value === threadId) {
      if (!fromUrl) syncRouteThreadId(threadId)
      return true
    }

    const previousThreadId = currentThreadId.value
    if (previousThreadId) {
      stopRunStreamSubscription(previousThreadId)
    }
    hideApprovalState()
    chatThreadsStore.setCurrentThreadId(threadId)
    if (!fromUrl) syncRouteThreadId(threadId)

    try {
      await fetchThreadMessages({ threadId })
      const targetThread = chatThreadsStore.threads.find((t) => t.id === threadId)
      if (targetThread?.agent_id && agentStore.selectedAgentId !== targetThread.agent_id) {
        await agentStore.selectAgent(targetThread.agent_id)
      }
      // 始终拉取 agent state：artifacts / todos / files 等历史回合依赖 checkpoint state
      await fetchAgentState(threadId)
      if (!restoreInterruptFromThreadState(threadId)) {
        await resumeActiveRunForThread(threadId)
      }
      return true
    } catch (e) {
      // 失败回滚到上一个 thread
      chatThreadsStore.setCurrentThreadId(previousThreadId)
      if (!fromUrl) syncRouteThreadId(previousThreadId || null)
      ElMessage.error(e?.message || '切换会话失败')
      return false
    }
  }

  // 新建对话
  const createNewChat = async () => {
    let agentId = agentStore.selectedAgentId
    if (!agentId) {
      await agentStore.initialize()
      agentId = agentStore.selectedAgentId
    }
    if (!agentId) {
      ElMessage.error('未选中智能体')
      return
    }
    const thread = await chatThreadsStore.createThread(agentId, '新的对话')
    if (thread?.id) {
      await selectThread(thread.id)
    }
  }

  // 切换会话
  const switchThread = async (threadId) => {
    if (!threadId) return
    if (threadId === currentThreadId.value) return
    await selectThread(threadId)
  }

  // 删除会话
  const deleteThread = async (threadId) => {
    if (!threadId) return
    try {
      await ElMessageBox.confirm('确定删除这个对话吗？删除后不可恢复。', '删除确认', {
        confirmButtonText: '删除',
        cancelButtonText: '取消',
        type: 'warning'
      })
    } catch {
      return
    }
    const wasCurrent = currentThreadId.value === threadId
    try {
      await chatThreadsStore.deleteThread(threadId)
      // 清理与该 thread 绑定的 pending 附件集合
      delete pendingAttachmentIdsByThread.value[threadId]
    } catch (e) {
      ElMessage.error(e?.message || '删除失败')
      return
    }
    if (wasCurrent) {
      const next = chatThreadsStore.threads[0]
      await selectThread(next?.id || null)
    }
  }

  // 重命名会话
  const renameThread = async (thread) => {
    if (!thread?.id) return
    try {
      const { value } = await ElMessageBox.prompt('请输入新标题', '重命名对话', {
        inputValue: thread.title || '新的对话',
        confirmButtonText: '确定',
        cancelButtonText: '取消'
      })
      const newTitle = value?.trim()
      if (newTitle && newTitle !== thread.title) {
        await chatThreadsStore.updateThread(thread.id, newTitle)
      }
    } catch {
      // 用户取消
    }
  }

  // 置顶/取消置顶
  const togglePinThread = async (thread) => {
    if (!thread?.id) return
    try {
      await chatThreadsStore.updateThread(thread.id, null, !thread.is_pinned)
      await chatThreadsStore.loadThreads()
    } catch (e) {
      ElMessage.error(e?.message || '置顶失败')
    }
  }

  // 会话列表时间格式化
  const formatThreadTime = (isoStr) => {
    if (!isoStr) return ''
    const normalized = /[zZ]$|[+-]\d{2}:\d{2}$/.test(isoStr) ? isoStr : isoStr + 'Z'
    const d = new Date(normalized)
    if (isNaN(d.getTime())) return ''
    const now = new Date()
    const diff = now - d
    if (diff < 60000) return '刚刚'
    if (diff < 3600000) return `${Math.floor(diff / 60000)}分钟前`
    if (diff < 86400000) return `${Math.floor(diff / 3600000)}小时前`
    if (diff < 604800000) return `${Math.floor(diff / 86400000)}天前`
    return d.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' })
  }

  // 页面可见时恢复流式
  const resumeCurrentRunForVisiblePage = async () => {
    if (typeof document !== 'undefined' && document.visibilityState !== 'visible') return
    const threadId = currentThreadId.value
    if (!threadId) return
    try {
      await resumeActiveRunForThread(threadId)
    } catch (e) {
      console.warn('Failed to resume current run after page became visible:', e)
    }
  }
  const handlePageVisibilityChange = () => {
    if (typeof document !== 'undefined' && document.visibilityState !== 'visible') return
    void resumeCurrentRunForVisiblePage()
  }

  watch(displayMessages, () => {
    if (isReplyLoading.value || isStreaming.value) {
      onScrollToBottom()
    }
  })

  onMounted(async () => {
    if (typeof document !== 'undefined') {
      document.addEventListener('visibilitychange', handlePageVisibilityChange)
    }
    try {
      await agentStore.initialize()
      await chatThreadsStore.loadThreads()

      const urlThreadId = route.query.thread_id
      let targetThreadId = null
      if (urlThreadId && typeof urlThreadId === 'string') {
        // URL 指定的 threadId 若不在已加载列表中，再尝试全量加载，避免
        // “跳转到老对话”时分页导致 404 / 看不到
        let exists = chatThreadsStore.threads.find((t) => t.id === urlThreadId)
        if (!exists && typeof chatThreadsStore.loadThreads === 'function') {
          try {
            await chatThreadsStore.loadThreads()
            exists = chatThreadsStore.threads.find((t) => t.id === urlThreadId)
          } catch (error) {
            console.warn('Failed to reload threads for URL thread_id:', error)
          }
        }
        if (exists) {
          targetThreadId = urlThreadId
        } else {
          // 无效 thread_id：清掉 URL 并落到首条 / null
          console.warn('[chat] URL thread_id 不存在，自动回退:', urlThreadId)
          syncRouteThreadId(null)
        }
      }
      if (!targetThreadId && chatThreadsStore.threads.length) {
        targetThreadId = chatThreadsStore.threads[0].id
      }

      // 统一入口选择会话（fromUrl: true 避免重复写 URL）
      if (targetThreadId) {
        syncingFromRoute.value = true
        try {
          await selectThread(targetThreadId, { fromUrl: !!urlThreadId })
        } finally {
          syncingFromRoute.value = false
        }
      }
    } catch (e) {
      console.error('init error', e)
    }
  })

  // 浏览器前进 / 后退：URL thread_id 变化时反向同步到 store / 加载消息
  // 但要排除"由我们自己 push/replace 引起"的 URL 变化（selectThread 的 fromUrl 流程已处理）
  watch(
    () => route.query.thread_id,
    async (newTid) => {
      if (!isChatRoute()) return
      if (syncingFromRoute.value) return
      const tid = (newTid || '').toString()
      if (!tid) {
        // 用户回退到 /chat，清空当前 thread
        if (currentThreadId.value) {
          stopRunStreamSubscription(currentThreadId.value)
          chatThreadsStore.setCurrentThreadId(null)
        }
        return
      }
      // 若 store 中已有这个 threadId 且当前 thread 已对得上，no-op
      if (currentThreadId.value === tid) return
      syncingFromRoute.value = true
      try {
        await selectThread(tid, { fromUrl: true })
      } finally {
        syncingFromRoute.value = false
      }
    }
  )

  watch(currentThreadId, async (newTid, oldTid) => {
    if (newTid && newTid !== oldTid) {
      // 总是拉取 agent_state：artifacts / todos / files 是历史回合也需要展示的内容
      // （旧实现只在面板打开时才拉，导致切对话看不到产物卡）
      await fetchAgentState(newTid)
    }
  })

  onUnmounted(() => {
    if (typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', handlePageVisibilityChange)
    }
    if (currentThreadId.value) {
      stopRunStreamSubscription(currentThreadId.value)
    }
  })
</script>

<style>
  /* 输入框容器：固定宽度，居中，跟消息区一致 */
  .chat-input-wrapper {
    max-width: 820px;
    margin: 0 auto;
    width: 100%;
  }

  .chat-composer-container {
    position: relative;
    z-index: 20;
  }

  .chat-composer-container.is-empty {
    position: absolute;
    top: 50%;
    left: 50%;
    width: min(48rem, calc(100% - clamp(2rem, 12vw, 10rem)));
    padding: 0;
    transform: translate(-50%, 1.6rem);
  }

  .chat-composer-container.is-empty .chat-input-wrapper {
    max-width: none;
  }

  .chat-composer-container.is-empty .elements-prompt-shell [data-slot='input-group'] {
    min-height: 7.5rem;
    border-color: var(--art-gray-300);
    border-radius: 1.2rem;
    box-shadow: none;
  }

  .chat-composer-container.is-empty .elements-prompt-shell .input-box {
    min-height: 7.35rem;
    padding: 1rem 1rem 0.75rem;
  }

  .chat-composer-container.is-empty .elements-prompt-shell .user-input {
    min-height: 3.5rem;
  }

  @media (max-width: 720px) {
    .chat-composer-container.is-empty {
      width: calc(100% - 2rem);
      transform: translate(-50%, 1rem);
    }
  }

  /* 附件按钮：无边框，hover 时浅灰背景（仿 Yuxi input-action-btn） */
  .input-attachment-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--el-text-color-secondary, #909399);
    cursor: pointer;
    transition: all 0.2s ease;
    padding: 0;
  }
  .input-attachment-btn:hover {
    background: var(--el-fill-color-light, #f5f7fa);
    color: var(--el-text-color-primary, #303133);
  }
  .input-attachment-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* 模型选择器与发送按钮之间的间距 */
  .actions-right-gap {
    width: 8px;
    flex-shrink: 0;
  }

  /* 空状态：欢迎 + 建议 */
  .chat-empty-state {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px 0;
  }
  .chat-empty-inner {
    width: 100%;
    max-width: 720px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
    text-align: center;
  }
  .chat-empty-welcome {
    width: 100%;
    align-items: center;
  }
  .chat-empty-welcome :deep(.welcome-title),
  .chat-empty-welcome :deep(.welcome-description) {
    text-align: center;
  }
  .chat-empty-prompts {
    width: 100%;
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 10px;
  }
  .chat-empty-prompts :deep(.el-prompts-item) {
    min-width: 0;
  }
  @media (max-width: 720px) {
    .chat-empty-prompts {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  /* Markdown 渲染 */
  .markdown-body {
    font-size: 14px;
    line-height: 1.75;
    word-break: break-word;
    overflow-wrap: anywhere;
    white-space: normal;
  }
  .markdown-body * {
    margin: 0;
  }
  .markdown-body p:last-child {
    margin-bottom: 0;
  }
  .markdown-body h1,
  .markdown-body h2 {
    margin: 12px 0 8px;
    font-weight: 600;
    font-size: 1rem;
  }
  .markdown-body h3,
  .markdown-body h4 {
    margin: 10px 0 6px;
    font-weight: 600;
    font-size: 0.95rem;
  }
  /* 列表：复盖 Tailwind v4 Preflight 的 list-style: none，让项目符号/数字序号显示 */
  .markdown-body ul,
  .markdown-body ol {
    list-style: revert;
    padding-left: 1.625rem;
    margin: 0;
  }
  .markdown-body ul {
    list-style-type: disc;
  }
  .markdown-body ol {
    list-style-type: decimal;
  }
  .markdown-body li {
    margin: 2px 0;
  }
  .markdown-body li > p {
    margin: 0.25rem 0;
  }
  .markdown-body li > ul,
  .markdown-body li > ol {
    margin: 4px 0;
    padding-left: 1.25rem;
  }
  .markdown-body pre {
    margin: 8px 0;
    padding: 12px 14px;
    border-radius: 8px;
    overflow: auto;
    max-width: 100%;
  }
  .markdown-body pre code {
    background: transparent;
    padding: 0;
  }
  .markdown-body code {
    font-family: monospace;
    font-size: 13px;
    padding: 1px 5px;
    border-radius: 4px;
  }
  .markdown-body blockquote {
    margin: 8px 0;
    padding: 0 0 0 1rem;
    border-left: 3px solid var(--art-gray-300);
  }
  .markdown-body table {
    display: block;
    width: max-content;
    max-width: 100%;
    min-width: 0;
    overflow-x: auto;
    border-collapse: collapse;
    margin: 8px 0;
  }
  .markdown-body th,
  .markdown-body td {
    padding: 6px 10px;
    border: 1px solid var(--art-gray-300);
  }
  .markdown-body hr {
    height: 1px;
    margin: 12px 0;
    border: 0;
    background: var(--art-gray-300);
  }
  .markdown-body img {
    max-width: 100%;
  }
  .markdown-body .katex-display {
    overflow-x: auto;
    overflow-y: hidden;
    max-width: 100%;
  }

  /* 图片放大预览：点击暗背景关闭 */
  :deep(.image-preview-dialog) {
    background: transparent;
    box-shadow: none;
  }
  :deep(.image-preview-dialog .el-dialog__header) {
    display: none;
  }
  :deep(.image-preview-dialog .el-dialog__body) {
    padding: 0;
  }
  .image-preview-img {
    max-width: 90vw;
    max-height: 90vh;
    display: block;
    cursor: zoom-out;
  }

  /* 生成中三点动画 */
  .loading-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--art-gray-500);
    animation: loading-bounce 1.4s infinite ease-in-out both;
  }
  .loading-dot:nth-child(1) {
    animation-delay: -0.32s;
  }
  .loading-dot:nth-child(2) {
    animation-delay: -0.16s;
  }
  @keyframes loading-bounce {
    0%,
    80%,
    100% {
      transform: scale(0);
      opacity: 0.5;
    }
    40% {
      transform: scale(1);
      opacity: 1;
    }
  }

  /* 会话列表折叠按钮 */
  .sidebar-toggle {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 40px;
    border: 1px solid var(--art-gray-200);
    border-radius: 6px;
    background: var(--art-color);
    cursor: pointer;
    color: var(--art-gray-500);
    transition: color 0.2s;
    z-index: 10;
  }
  .sidebar-toggle:hover {
    color: var(--art-primary-color);
  }

  /* 对话标题栏按钮 */
  .header-panel-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    color: var(--art-gray-600);
    background: transparent;
    border: 1px solid transparent;
    cursor: pointer;
    transition: all 0.2s ease;
  }
  .header-panel-btn:hover {
    color: var(--art-gray-900);
    background: var(--art-gray-50);
  }
  .header-panel-btn.active {
    color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);
    border-color: var(--el-color-primary-light-7);
  }

  /* 附件预览 chips */
  .chat-attachment-preview {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 4px 0;
  }
  /* 用户消息内附件 */
  .user-msg-attachments {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .user-msg-att-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    border-radius: 6px;
    background: var(--art-gray-50, rgba(0, 0, 0, 0.02));
    border: 1px solid var(--art-gray-200);
    max-width: 200px;
  }
  .chat-attachment-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    font-size: 12px;
    border-radius: 6px;
    background: var(--art-gray-50);
    border: 1px solid var(--art-gray-200);
  }
  .chat-attachment-chip .chip-name {
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chat-attachment-chip .chip-remove {
    cursor: pointer;
    font-size: 14px;
    color: var(--art-gray-500);
  }
  .chat-attachment-chip .chip-parsed {
    font-size: 10px;
    padding: 0 4px;
    border-radius: 3px;
    background: var(--el-color-success-light-9);
    color: var(--el-color-success);
  }
  .chat-attachment-chip .chip-remove:hover {
    color: var(--el-color-danger);
  }

  /* 智能体下拉 */
  .agent-dropdown-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
  }
  .agent-dropdown-avatar {
    width: 18px;
    height: 18px;
    border-radius: 50%;
    object-fit: cover;
    flex-shrink: 0;
  }
  .thread-gradient-avatar {
    flex-shrink: 0;
    box-shadow:
      inset 0 0 0 1px rgb(255 255 255 / 32%),
      0 2px 8px rgb(15 23 42 / 10%);
  }
  .agent-dropdown-name {
    font-size: 13px;
    font-weight: 500;
    line-height: 1.4;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* 会话列表操作按钮 */
  .thread-action-btn {
    opacity: 0;
    transition: opacity 0.2s;
    flex-shrink: 0;
    margin-left: 0 !important;
  }
  .thread-row:hover .thread-action-btn {
    opacity: 1;
  }
  .thread-action-btn + .thread-action-btn {
    margin-left: -4px !important;
  }

  /* 状态面板 side-panel（悬浮小卡片） */
  .side-panel {
    flex: 0 0 auto;
    overflow: hidden;
    background: var(--art-color);
    border: 1px solid var(--art-gray-300);
    border-radius: 10px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.06);
    z-index: 20;
    min-width: 0;
    opacity: 0;
    pointer-events: none;
    transform: translateX(10px);
    transition:
      flex-basis 0.3s cubic-bezier(0.4, 0, 0.2, 1),
      opacity 0.3s cubic-bezier(0.4, 0, 0.2, 1),
      transform 0.3s cubic-bezier(0.4, 0, 0.2, 1);
  }
  .side-panel.is-visible {
    opacity: 1;
    pointer-events: auto;
    transform: translateX(0);
  }
  .side-panel--state {
    position: absolute;
    top: 8px;
    right: 12px;
    width: 340px;
    min-width: 0;
    max-width: calc(100% - 24px);
    max-height: calc(100% - 16px);
    height: auto;
    overflow: auto;
    margin: 0;
    z-index: 26;
    box-shadow:
      0 12px 28px rgba(0, 0, 0, 0.12),
      0 2px 8px rgba(0, 0, 0, 0.06);
  }
  .state-panel {
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .state-panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 14px 0;
    background: transparent;
    flex-shrink: 0;
  }
  .state-panel-header-actions {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .state-panel-title {
    min-width: 0;
    font-size: 14px;
    font-weight: 500;
    color: var(--art-gray-600);
  }
  .state-panel-summary {
    font-size: 12px;
    color: var(--art-gray-500);
    flex-shrink: 0;
  }
  .state-refresh-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: 6px;
    color: var(--art-gray-500);
    background: transparent;
    cursor: pointer;
  }
  .state-refresh-btn:hover:not(:disabled) {
    color: var(--el-color-primary);
    background: var(--art-gray-50);
  }
  .state-refresh-btn:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .state-refresh-btn .is-spinning {
    animation: state-spin 1s linear infinite;
  }
  @keyframes state-spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
  .state-panel-body {
    flex: 1;
    min-height: 0;
    padding: 8px 14px 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    overflow: auto;
  }
  .state-panel-empty {
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--art-gray-50);
    color: var(--art-gray-500);
    font-size: 13px;
    text-align: center;
  }
  .state-panel-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--el-color-danger-light-9);
    color: var(--el-color-danger);
    font-size: 13px;
  }
  .state-panel-error button {
    flex-shrink: 0;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--el-color-primary);
    cursor: pointer;
  }
  .state-overview {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 8px;
  }
  .state-overview-item {
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 8px 4px;
    border: 1px solid var(--art-gray-200);
    border-radius: 8px;
    background: var(--art-gray-50);
  }
  .state-overview-item strong {
    color: var(--art-gray-900);
    font-size: 15px;
    font-variant-numeric: tabular-nums;
  }
  .state-overview-item span {
    overflow: hidden;
    color: var(--art-gray-500);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .state-panel-artifacts {
    margin-top: 4px;
  }

  /* 文件工作区 side-panel */
  .side-panel--file {
    position: relative;
    height: auto;
    max-height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--art-color);
    border: 1px solid var(--art-gray-300);
    border-radius: 10px;
    box-shadow:
      0 12px 28px rgba(0, 0, 0, 0.12),
      0 2px 8px rgba(0, 0, 0, 0.06);
  }
  .side-panel--file.is-visible {
    min-width: 320px;
  }
  .file-panel-empty {
    padding: 24px;
    text-align: center;
    font-size: 13px;
    color: var(--art-gray-500);
  }
  .side-panel--file .file-workspace {
    height: 100%;
    border: none;
    border-radius: 0;
    box-shadow: none;
  }

  @media (max-width: 768px) {
    .side-panel--state {
      right: 12px;
      width: min(320px, calc(100% - 24px));
    }
    .side-panel--file.is-visible {
      min-width: 0;
      max-width: calc(100% - 24px);
    }
  }
</style>
