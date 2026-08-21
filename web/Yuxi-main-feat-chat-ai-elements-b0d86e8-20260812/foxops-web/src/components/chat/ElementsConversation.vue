<template>
  <div
    class="elements-conversation-shell h-full min-h-0 flex-1"
    :class="{ 'is-empty': showEmptyState }"
  >
    <Conversation
      ref="conversationRef"
      aria-label="对话消息列表"
      class="elements-conversation h-full min-h-0 w-full"
    >
      <ConversationContent class="elements-conversation-content">
        <template v-if="!showEmptyState">
          <Message
            v-for="(message, index) in elementMessages"
            :key="message.id"
            :from="message.role"
            class="elements-message"
          >
            <template v-if="message.role === 'user'">
              <div class="elements-message__main elements-message__main--user">
                <MessageContent class="elements-message__content elements-message__content--user">
                  <template
                    v-for="(part, partIndex) in message.parts"
                    :key="`${message.id}-${part.type}-${partIndex}`"
                  >
                    <div v-if="part.type === 'error'" class="elements-message__notice">
                      <template v-if="part.payload.isStoppedByUser">
                        <span>你停止生成了本次回答</span>
                        <button type="button" @click="emit('retry', getRawItem(index))">
                          重新生成
                        </button>
                      </template>
                      <template v-else>
                        {{ part.payload.errorMessage || part.payload.errorType }}
                      </template>
                    </div>
                    <img
                      v-else-if="part.type === 'image'"
                      :src="`data:${part.payload.mime};base64,${part.payload.content}`"
                      alt="用户上传的图片"
                      class="elements-message__image"
                      @click="emit('preview-image', part.payload)"
                    />
                    <div v-else-if="part.type === 'attachments'" class="user-msg-attachments">
                      <div
                        v-for="attachment in part.payload.attachments"
                        :key="attachment.file_id || attachment.name"
                        class="user-msg-att-item"
                      >
                        <ArtSvgIcon icon="ri:file-3-line" class="shrink-0 text-sm text-g-500" />
                        <div class="min-w-0">
                          <div class="truncate text-xs font-medium">
                            {{ attachment.file_name || attachment.name }}
                          </div>
                          <div v-if="attachment.file_size" class="text-xs text-g-400">
                            {{ attachment.file_size }}
                          </div>
                        </div>
                      </div>
                    </div>
                    <MentionTextRenderer
                      v-else-if="part.type === 'text'"
                      :content="part.payload.content"
                      class="mention-content"
                    />
                  </template>
                </MessageContent>
                <div class="elements-message__footer elements-message__footer--user">
                  <span v-if="message.meta.time" class="elements-message__time">
                    {{ message.meta.time }}
                  </span>
                  <button
                    type="button"
                    class="elements-message__action"
                    title="复制"
                    @click="emit('copy', getRawItem(index)?.content || '')"
                  >
                    <ArtSvgIcon icon="ri:file-copy-line" />
                  </button>
                </div>
              </div>
            </template>

            <template v-else>
              <MessageAvatar
                :src="message.meta.avatar || assistantAvatar"
                :name="message.meta.sender || assistantName"
              />
              <div class="elements-message__main">
                <div class="elements-message__header">
                  <span class="elements-message__sender">
                    {{ message.meta.sender || assistantName }}
                  </span>
                  <span v-if="message.meta.time" class="elements-message__time">
                    · {{ message.meta.time }}
                  </span>
                </div>
                <MessageContent
                  class="elements-message__content elements-message__content--assistant"
                >
                  <template
                    v-for="(part, partIndex) in message.parts"
                    :key="`${message.id}-${part.type}-${partIndex}`"
                  >
                    <Reasoning
                      v-if="part.type === 'reasoning'"
                      :is-streaming="isActiveConversationItem(getRawItem(index))"
                      class="elements-reasoning"
                    >
                      <ReasoningTrigger />
                      <ReasoningContent
                        :content="part.payload.reasoning"
                        class="elements-reasoning__content"
                      />
                    </Reasoning>
                    <ToolCallsGroupComponent
                      v-else-if="part.type === 'tool-group'"
                      :tool-calls="part.payload.toolCalls"
                      :is-active="isActiveConversationItem(getRawItem(index))"
                    />
                    <MessageResponse
                      v-else-if="part.type === 'text'"
                      :content="part.payload.content"
                      class="elements-message__response"
                    />
                    <RefsComponent
                      v-else-if="part.type === 'sources'"
                      :sources="part.payload.sources"
                      class="elements-message__sources"
                    />
                    <ArtifactsCard
                      v-else-if="part.type === 'artifacts'"
                      :artifacts="part.payload.artifacts"
                      :thread-id="threadId"
                      class="elements-message__artifacts"
                    />
                    <div v-else-if="part.type === 'error'" class="elements-message__notice">
                      <template v-if="part.payload.isStoppedByUser">
                        <span>你停止生成了本次回答</span>
                        <button type="button" @click="emit('retry', getRawItem(index))">
                          重新生成
                        </button>
                      </template>
                      <template v-else>
                        {{ part.payload.errorMessage || part.payload.errorType }}
                      </template>
                    </div>
                  </template>
                </MessageContent>
                <div class="elements-message__footer">
                  <button
                    type="button"
                    class="elements-message__action"
                    title="复制"
                    @click="emit('copy', getRawItem(index)?.content || '')"
                  >
                    <ArtSvgIcon icon="ri:file-copy-line" />
                  </button>
                  <MessageFeedback
                    v-if="message.meta.msgId"
                    :message="{ id: message.meta.msgId, feedback: message.meta.feedback }"
                  />
                  <button
                    v-if="message.meta.msgId && !isStreaming"
                    type="button"
                    class="elements-message__action"
                    title="重新生成"
                    @click="emit('retry', getRawItem(index))"
                  >
                    <ArtSvgIcon icon="ri:refresh-line" />
                  </button>
                  <span v-if="message.meta.modelName" class="elements-message__model">
                    {{ message.meta.modelName }}
                  </span>
                </div>
              </div>
            </template>
          </Message>

          <Message v-if="showReplyLoading" from="assistant" class="elements-message">
            <MessageAvatar :src="assistantAvatar" :name="assistantName" />
            <div class="elements-message__main">
              <div class="elements-message__header">
                <span class="elements-message__sender">{{ assistantName }}</span>
              </div>
              <div class="elements-loading">
                <Loader :size="17" />
                <Shimmer class="text-sm" :duration="1.2">正在思考…</Shimmer>
              </div>
            </div>
          </Message>
        </template>
      </ConversationContent>

      <template #overlay>
        <ConversationScrollButton class="elements-scroll-button" />
      </template>
    </Conversation>

    <div v-if="showEmptyState" class="elements-empty-state__glow" aria-hidden="true"></div>
    <div v-if="showEmptyState" class="elements-empty-state__overlay">
      <ConversationEmptyState class="elements-empty-state">
        <div class="elements-empty-state__heading">
          <div class="elements-empty-state__body">
            <h2 class="elements-empty-state__title">今天想一起做点什么？</h2>
            <p class="elements-empty-state__description">{{ greetingText }}</p>
          </div>
        </div>
        <Suggestions v-if="starterPrompts.length" class="elements-empty-state__suggestions">
          <Suggestion
            v-for="prompt in starterPrompts"
            :key="prompt.key"
            :suggestion="prompt.label"
            class="elements-empty-state__prompt"
            @click="emit('prompt-click', prompt)"
          />
        </Suggestions>
      </ConversationEmptyState>
    </div>
  </div>
</template>

<script setup>
  import { computed, nextTick, ref } from 'vue'
  import {
    Conversation,
    ConversationContent,
    ConversationEmptyState,
    ConversationScrollButton
  } from '@/components/ai-elements/conversation'
  import {
    Message,
    MessageAvatar,
    MessageContent,
    MessageResponse
  } from '@/components/ai-elements/message'
  import { Reasoning, ReasoningContent, ReasoningTrigger } from '@/components/ai-elements/reasoning'
  import { Suggestion, Suggestions } from '@/components/ai-elements/suggestion'
  import { Loader } from '@/components/ai-elements/loader'
  import { Shimmer } from '@/components/ai-elements/shimmer'
  import MessageFeedback from '@/components/chat/MessageFeedback.vue'
  import ArtifactsCard from '@/components/chat/ArtifactsCard.vue'
  import RefsComponent from '@/components/chat/RefsComponent.vue'
  import ToolCallsGroupComponent from '@/components/ToolCallsGroupComponent.vue'
  import MentionTextRenderer from '@/components/common/MentionTextRenderer.vue'
  import { toElementsMessages } from '@/utils/chat/elementsAdapter'

  const props = defineProps({
    items: { type: Array, default: () => [] },
    isStreaming: { type: Boolean, default: false },
    isReplyLoading: { type: Boolean, default: false },
    isLoadingMessages: { type: Boolean, default: false },
    greetingText: { type: String, default: '' },
    starterPrompts: { type: Array, default: () => [] },
    userName: { type: String, default: '用户' },
    assistantName: { type: String, default: '智能助手' },
    assistantAvatar: { type: String, default: '' },
    threadId: { type: String, default: '' }
  })

  const emit = defineEmits(['copy', 'retry', 'prompt-click', 'preview-image'])
  const conversationRef = ref(null)
  const elementMessages = computed(() => toElementsMessages(props.items))
  const lastConversationIndex = computed(() =>
    props.items.reduce((max, item) => Math.max(max, item?._convIdx ?? -1), -1)
  )
  const showEmptyState = computed(
    () => !props.items.length && !props.isReplyLoading && !props.isLoadingMessages
  )
  const showReplyLoading = computed(() => {
    const lastMessage = elementMessages.value.at(-1)
    return (
      props.isReplyLoading &&
      !props.isLoadingMessages &&
      (!lastMessage || lastMessage.role === 'user')
    )
  })

  const getRawItem = (index) => props.items[index] || null
  const isActiveConversationItem = (item) =>
    props.isStreaming && item?._convIdx === lastConversationIndex.value

  const resolveScrollElement = () => {
    const root = conversationRef.value?.$el || conversationRef.value
    if (!root?.querySelector) return null
    const nodes = root.querySelectorAll('div')
    for (const node of nodes) {
      const style = typeof getComputedStyle === 'function' ? getComputedStyle(node) : null
      if (style && (style.overflowY === 'auto' || style.overflow === 'auto')) return node
    }
    return root
  }

  const scrollToBottom = async (instant = false) => {
    await nextTick()
    const element = resolveScrollElement()
    if (!element || typeof element.scrollTo !== 'function') return
    element.scrollTo({ top: element.scrollHeight, behavior: instant ? 'auto' : 'smooth' })
  }

  defineExpose({ scrollToBottom })
</script>

<style scoped>
  .elements-conversation-shell {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }

  .elements-conversation-content {
    position: relative;
    width: min(56rem, calc(100% - clamp(2rem, 8vw, 7rem)));
    box-sizing: border-box;
    height: 100%;
    min-height: 100%;
    gap: 1.75rem;
    padding: 1.25rem 0 2rem;
    margin: 0 auto;
  }

  .elements-empty-state__glow {
    position: absolute;
    top: 50%;
    left: 50%;
    z-index: 0;
    width: min(75rem, 140vw);
    height: min(50rem, 95vh);
    pointer-events: none;
    background: radial-gradient(
      ellipse at center,
      rgb(232 240 254 / 80%) 0%,
      rgb(220 233 255 / 38%) 34%,
      transparent 60%
    );
    transform: translate(-50%, -48%);
  }

  :global(.dark) .elements-empty-state__glow {
    width: min(87.5rem, 150vw);
    height: min(56.25rem, 105vh);
    background: radial-gradient(
      ellipse at center,
      rgb(168 199 250 / 25%) 0%,
      rgb(96 165 250 / 10%) 36%,
      transparent 60%
    );
    transform: translate(-50%, -42%);
  }

  .elements-empty-state__overlay {
    position: absolute;
    top: calc(50% + 0.25rem);
    left: 50%;
    z-index: 1;
    width: min(48rem, calc(100% - clamp(2rem, 12vw, 10rem)));
    transform: translate(-50%, -100%);
  }

  .elements-empty-state {
    position: static;
    width: 100%;
    height: auto !important;
    gap: 0.75rem;
    justify-content: flex-start;
    align-items: flex-start;
    min-height: 0;
    padding: 0;
    margin: 0;
    text-align: left;
    transform: none;
  }

  .elements-empty-state__heading {
    display: block;
    width: 100%;
  }

  .elements-empty-state__body {
    min-width: 0;
  }

  .elements-empty-state__title {
    margin: 0;
    color: var(--art-gray-900);
    font-size: clamp(1.55rem, 2.5vw, 2rem);
    font-weight: 650;
    letter-spacing: -0.025em;
  }

  .elements-empty-state__description {
    max-width: 38rem;
    margin: 0.35rem 0 0;
    color: var(--art-gray-500);
    font-size: 0.82rem;
    line-height: 1.65;
  }

  .elements-empty-state__suggestions {
    width: 100%;
    padding-top: 0.2rem;
  }

  .elements-empty-state__prompt {
    height: 2.15rem;
    max-width: none;
    color: var(--art-gray-600);
    font-size: 0.75rem;
  }

  .elements-message {
    max-width: 100%;
    align-items: flex-start;
    gap: 0.65rem;
  }

  .elements-message.is-user {
    width: min(76%, 40rem);
  }

  .elements-message__main {
    display: flex;
    min-width: 0;
    flex: 1;
    flex-direction: column;
    gap: 0.4rem;
  }

  .elements-message__main--user {
    align-items: flex-end;
  }

  .elements-message__header {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    min-height: 1.25rem;
    color: var(--art-gray-500);
    font-size: 0.75rem;
  }

  .elements-message__sender {
    color: var(--art-gray-700);
    font-weight: 600;
  }

  .elements-message__content {
    width: 100%;
  }

  .elements-message__content--assistant {
    gap: 0.8rem;
    overflow: visible;
    font-size: 0.9rem;
    line-height: 1.7;
  }

  .elements-message__content--user {
    width: fit-content;
    max-width: 100%;
    border-radius: 1.1rem;
    padding: 0.7rem 1rem;
    font-size: 0.9rem;
    line-height: 1.6;
    overflow-wrap: anywhere;
  }

  .elements-message__response {
    width: 100%;
    overflow-wrap: anywhere;
  }

  .elements-message__response :deep(p) {
    margin-block: 0 0.75rem;
  }

  .elements-message__response :deep(ul),
  .elements-message__response :deep(ol) {
    margin-block: 0.5rem;
    padding-left: 1.4rem;
  }

  .elements-message__response :deep(table) {
    display: block;
    max-width: 100%;
    overflow-x: auto;
  }

  .elements-reasoning {
    width: 100%;
    margin-bottom: 0;
  }

  .elements-reasoning__content {
    max-height: 13.75rem;
    margin-top: 0.65rem;
    overflow: auto;
    border-left: 2px solid var(--art-gray-200);
    padding-left: 1rem;
    overscroll-behavior: contain;
  }

  .elements-message__sources,
  .elements-message__artifacts {
    margin-top: 0.15rem;
  }

  .elements-message__notice {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--el-color-danger, #f56c6c);
    font-size: 0.75rem;
  }

  .elements-message__notice button {
    border: 0;
    background: transparent;
    color: var(--el-color-primary, #409eff);
    cursor: pointer;
    padding: 0;
  }

  .elements-message__image {
    max-width: 100%;
    max-height: 15rem;
    border-radius: 0.75rem;
    cursor: zoom-in;
    object-fit: contain;
  }

  .elements-message__footer {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-height: 1.55rem;
    color: var(--art-gray-400);
    opacity: 0;
    transition: opacity 120ms ease;
  }

  .elements-message:hover .elements-message__footer,
  .elements-message:focus-within .elements-message__footer,
  .elements-message__footer--user {
    opacity: 1;
  }

  .elements-message__footer--user {
    justify-content: flex-end;
    padding-right: 0.25rem;
  }

  .elements-message__action {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.55rem;
    height: 1.55rem;
    border: 0;
    border-radius: 0.4rem;
    background: transparent;
    color: var(--art-gray-500);
    cursor: pointer;
    padding: 0;
  }

  .elements-message__action:hover {
    background: var(--art-gray-100);
    color: var(--art-gray-700);
  }

  .elements-message__model {
    margin-left: auto;
    font-size: 0.7rem;
  }

  .elements-message__time {
    font-size: 0.7rem;
  }

  .elements-loading {
    display: inline-flex;
    width: fit-content;
    align-items: center;
    gap: 0.5rem;
    color: var(--art-gray-500);
  }

  .elements-scroll-button {
    bottom: 1rem;
  }

  .user-msg-attachments {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .user-msg-att-item {
    display: flex;
    max-width: 14rem;
    align-items: center;
    gap: 0.4rem;
    border: 1px solid var(--art-gray-200);
    border-radius: 0.5rem;
    padding: 0.35rem 0.5rem;
  }

  @media (max-width: 720px) {
    .elements-conversation-content {
      width: calc(100% - 1.5rem);
    }

    .elements-empty-state__glow {
      width: 150vw;
      height: 70vh;
    }

    .elements-empty-state__overlay {
      width: calc(100% - 2rem);
    }

    .elements-message.is-user {
      width: min(88%, 40rem);
    }

    .elements-message__footer {
      opacity: 1;
    }
  }
</style>
