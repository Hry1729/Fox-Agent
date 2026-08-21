<template>
  <ElDialog v-model="visible" title="用户反馈详情" width="960px" destroy-on-close>
    <ElSegmented v-model="feedbackFilter" :options="feedbackOptions" class="filter-bar" />

    <div v-loading="loadingFeedbacks" class="feedback-body">
      <div v-if="!loadingFeedbacks && feedbacks.length" class="feedback-grid">
        <div v-for="feedback in feedbacks" :key="feedback.id" class="feedback-card">
          <div class="card-header">
            <div class="user-info">
              <FallbackAvatar
                :src="feedback.avatar"
                :default-src="getFeedbackDefaultAvatarSrc(feedback)"
                :name="feedback.username"
                :seed="feedback.uid || feedback.username"
                kind="user"
                :size="32"
                shape="circle"
                :alt="feedback.username"
              />
              <span class="username">{{ feedback.username || '未知用户' }}</span>
            </div>
            <ElTag :type="feedback.rating === 'like' ? 'success' : 'danger'" size="small">
              {{ feedback.rating === 'like' ? '点赞' : '点踩' }}
            </ElTag>
          </div>

          <div class="card-content">
            <div v-if="feedback.conversation_title" class="info-block">
              <span class="info-label">标题</span>
              <p
                class="info-text"
                :class="{ collapsed: !isExpanded(feedback.id, 'conversation') }"
              >
                {{ feedback.conversation_title }}
              </p>
              <ElButton
                v-if="shouldShowConversationExpand(feedback.conversation_title)"
                link
                type="primary"
                size="small"
                @click="toggleExpand(feedback.id, 'conversation')"
              >
                {{ isExpanded(feedback.id, 'conversation') ? '收起' : '展开' }}
              </ElButton>
            </div>

            <div class="info-block">
              <span class="info-label">智能体</span>
              <span class="info-value">{{ feedback.agent_id || '-' }}</span>
            </div>

            <div class="info-block">
              <span class="info-label">消息</span>
              <p
                class="message-text"
                :class="{ collapsed: !isExpanded(feedback.id, 'message') }"
              >
                {{ feedback.message_content }}
              </p>
              <ElButton
                v-if="shouldShowMessageExpand(feedback.message_content)"
                link
                type="primary"
                size="small"
                @click="toggleExpand(feedback.id, 'message')"
              >
                {{ isExpanded(feedback.id, 'message') ? '收起' : '展开全部' }}
              </ElButton>
            </div>

            <div v-if="feedback.reason" class="info-block reason-block">
              <span class="info-label">反馈原因</span>
              <p class="reason-text">{{ feedback.reason }}</p>
            </div>
          </div>

          <div class="card-footer">
            <ArtSvgIcon icon="ri:time-line" class="time-icon" />
            <span>{{ formatDateTime(feedback.created_at) }}</span>
          </div>
        </div>
      </div>

      <ElEmpty v-else-if="!loadingFeedbacks" description="暂无反馈数据" />
    </div>
  </ElDialog>
</template>

<script setup lang="ts">
  import { dashboardApi } from '@/api/dashboard'
  import FallbackAvatar from '@/components/common/FallbackAvatar.vue'
  import { generatePixelAvatar } from '@/utils/pixelAvatar'

  defineOptions({ name: 'FeedbackDialog' })

  interface FeedbackItem {
    id: string | number
    avatar?: string
    username?: string
    uid?: string
    rating?: 'like' | 'dislike' | string
    conversation_title?: string
    agent_id?: string
    message_content?: string
    reason?: string
    created_at?: string
  }

  const CONFIG = {
    MESSAGE_MAX_LINES: 8,
    CONVERSATION_MAX_CHARS: 60,
    AVG_CHARS_PER_LINE: 30
  }

  const visible = ref(false)
  const feedbacks = ref<FeedbackItem[]>([])
  const loadingFeedbacks = ref(false)
  const feedbackFilter = ref('all')
  const feedbackOptions = [
    { label: '全部', value: 'all' },
    { label: '点赞', value: 'like' },
    { label: '点踩', value: 'dislike' }
  ]

  const expandedStates = ref(new Map<string, boolean>())

  const expandKey = (id: string | number, field: 'message' | 'conversation') =>
    `${id}-${field}`

  const isExpanded = (id: string | number, field: 'message' | 'conversation') =>
    expandedStates.value.get(expandKey(id, field)) ?? false

  const toggleExpand = (id: string | number, field: 'message' | 'conversation') => {
    const key = expandKey(id, field)
    expandedStates.value.set(key, !isExpanded(id, field))
  }

  const estimateLines = (text?: string) => {
    if (!text) return 0
    return Math.ceil(text.length / CONFIG.AVG_CHARS_PER_LINE)
  }

  const shouldShowMessageExpand = (content?: string) =>
    estimateLines(content) > CONFIG.MESSAGE_MAX_LINES

  const shouldShowConversationExpand = (title?: string) =>
    Boolean(title && title.length > CONFIG.CONVERSATION_MAX_CHARS)

  const getFeedbackDefaultAvatarSrc = (feedback: FeedbackItem) =>
    feedback.uid ? generatePixelAvatar(feedback.uid) : ''

  const formatDateTime = (value?: string) => {
    if (!value) return '-'
    const date = new Date(value)
    if (Number.isNaN(date.getTime())) return value
    return date.toLocaleString('zh-CN', { hour12: false })
  }

  const loadFeedbacks = async () => {
    loadingFeedbacks.value = true
    try {
      const response = (await dashboardApi.getFeedbacks({
        rating: feedbackFilter.value === 'all' ? undefined : feedbackFilter.value
      })) as FeedbackItem[]
      feedbacks.value = Array.isArray(response) ? response : []
      expandedStates.value.clear()
    } catch (error) {
      console.error('加载反馈列表失败:', error)
      ElMessage.error('加载反馈列表失败，请稍后重试')
      feedbacks.value = []
    } finally {
      loadingFeedbacks.value = false
    }
  }

  const open = () => {
    visible.value = true
    loadFeedbacks()
  }

  defineExpose({ open })

  watch(feedbackFilter, () => {
    if (visible.value) {
      loadFeedbacks()
    }
  })
</script>

<style scoped>
  .filter-bar {
    margin-bottom: 16px;
  }

  .feedback-body {
    min-height: 200px;
  }

  .feedback-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 16px;
    max-height: 560px;
    overflow-y: auto;
    padding-right: 4px;
  }

  .feedback-card {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--art-gray-200);
    border-radius: 8px;
    background: var(--art-gray-0);
  }

  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--art-gray-150);
    background: var(--art-gray-50);
  }

  .user-info {
    display: flex;
    gap: 8px;
    align-items: center;
    min-width: 0;
  }

  .username {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 500;
    color: var(--art-gray-900);
  }

  .card-content {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 10px;
    padding: 14px 16px;
  }

  .info-block {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .info-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--art-gray-600);
  }

  .info-value {
    font-size: 13px;
    color: var(--art-gray-800);
    word-break: break-all;
  }

  .info-text,
  .message-text {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--art-gray-800);
    word-break: break-word;
  }

  .message-text {
    padding: 10px;
    background: var(--art-gray-50);
    border-radius: 6px;
  }

  .info-text.collapsed,
  .message-text.collapsed {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  .message-text.collapsed {
    -webkit-line-clamp: 8;
    line-clamp: 8;
  }

  .reason-block .reason-text {
    margin: 0;
    padding: 10px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--art-gray-800);
    background: var(--el-color-warning-light-9);
    border-left: 3px solid var(--el-color-warning);
    border-radius: 6px;
    word-break: break-word;
  }

  .card-footer {
    display: flex;
    gap: 4px;
    align-items: center;
    padding: 8px 16px;
    font-size: 11px;
    color: var(--art-gray-500);
    border-top: 1px solid var(--art-gray-150);
    background: var(--art-gray-50);
  }

  .time-icon {
    font-size: 14px;
  }
</style>
