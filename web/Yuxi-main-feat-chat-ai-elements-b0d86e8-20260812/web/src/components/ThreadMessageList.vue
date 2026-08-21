<template>
  <div class="thread-message-list">
    <template v-for="(conv, convIndex) in conversations" :key="`conv-${convIndex}`">
      <template v-for="displayItem in getDisplayItems(conv)" :key="displayItem.key">
        <AgentMessageComponent
          v-if="displayItem.type === 'message'"
          :message="displayItem.message"
          :show-refs="false"
          :hide-tool-calls="true"
          :mention="{}"
        />
        <ToolCallsGroupComponent v-else :tool-calls="displayItem.toolCalls" />
      </template>
    </template>
    <div v-if="conversations.length === 0" class="thread-message-list-empty">暂无消息</div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import AgentMessageComponent from '@/components/AgentMessageComponent.vue'
import ToolCallsGroupComponent from '@/components/ToolCallsGroupComponent.vue'
import { MessageProcessor } from '@/utils/messageProcessor'
import { getConversationDisplayItems } from '@/utils/messageGrouping'

const props = defineProps({
  messages: {
    type: Array,
    default: () => []
  },
  enrichToolCalls: {
    type: Function,
    default: null
  }
})

const conversations = computed(() =>
  MessageProcessor.convertServerHistoryToMessages(props.messages)
)

const getDisplayItems = (conv) =>
  getConversationDisplayItems(
    conv,
    props.enrichToolCalls ? { enrichToolCalls: props.enrichToolCalls } : {}
  )
</script>

<style lang="less" scoped>
.thread-message-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.thread-message-list-empty {
  padding: 24px 0;
  text-align: center;
  color: var(--gray-500);
  font-size: 13px;
}
</style>
