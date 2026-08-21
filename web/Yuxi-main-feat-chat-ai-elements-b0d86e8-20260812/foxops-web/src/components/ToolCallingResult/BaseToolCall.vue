<template>
  <Tool
    :default-open="defaultExpanded"
    :class="[
      'tool-call-display',
      { 'is-timeline': isTimeline, 'is-card': !isTimeline }
    ]"
  >
    <!-- 有自定义 header 时保留领域标题，仍用 Elements 折叠触发器 -->
    <CollapsibleTrigger
      v-if="hasCustomHeader"
      class="flex w-full items-center justify-between gap-4 p-3"
    >
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <WrenchIcon class="size-4 shrink-0 text-muted-foreground" />
        <div class="tool-header-custom min-w-0 flex-1 text-left text-sm font-medium">
          <slot
            v-if="$slots.header"
            name="header"
            :tool-call="toolCall"
            :tool-name="toolName"
          />
          <template v-else>
            <slot
              v-if="isCompleted"
              name="header-success"
              :tool-name="toolName"
              :result-content="resultContent"
            >
              工具&nbsp;<span class="tool-name">{{ toolName }}</span>&nbsp;执行完成
            </slot>
            <slot
              v-else-if="isFailed"
              name="header-error"
              :tool-name="toolName"
              :error-message="toolCall.error_message"
            >
              工具&nbsp;<span class="tool-name">{{ toolName }}</span>&nbsp;执行失败
            </slot>
            <slot v-else name="header-running" :tool-name="toolName">
              正在调用工具:&nbsp;<span class="tool-name">{{ toolName }}</span>
            </slot>
          </template>
        </div>
        <ToolStatusBadge :state="elementsProps.state" />
      </div>
      <ChevronDownIcon
        class="size-4 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180"
      />
    </CollapsibleTrigger>
    <ToolHeader
      v-else
      :title="toolName"
      :type="elementsProps.type"
      :state="elementsProps.state"
      class="tool-header-elements"
    />
    <ToolContent>
      <div
        v-if="!hideParams && ($slots.params || hasParams)"
        class="tool-body-block params-block"
      >
        <slot name="params" :tool-call="toolCall" :args="formattedArgs">
          <ToolInput :input="parsedInput" />
        </slot>
      </div>
      <div
        v-if="$slots.result || hasResult || forceShowResult"
        class="tool-body-block result-block"
      >
        <slot name="result" :tool-call="toolCall" :result-content="resultContent">
          <ToolOutput
            :output="parsedResultData"
            :error-text="elementsProps.errorText"
          />
        </slot>
      </div>
    </ToolContent>
  </Tool>
</template>

<script setup>
import { computed, useSlots } from 'vue'
import { ChevronDownIcon, WrenchIcon } from '@lucide/vue'
import { CollapsibleTrigger } from '@/components/ui/collapsible'
import {
  Tool,
  ToolContent,
  ToolHeader,
  ToolInput,
  ToolOutput,
  ToolStatusBadge
} from '@/components/ai-elements/tool'
import { useAgentStore } from '@/store/modules/agent'
import { getToolCallId, parseToolCallArgs } from './toolRegistry'
import { toElementsToolProps } from '@/utils/chat/toolElementsAdapter'

const props = defineProps({
  toolCall: {
    type: Object,
    required: true
  },
  defaultExpanded: {
    type: Boolean,
    default: false
  },
  hideParams: {
    type: Boolean,
    default: false
  },
  appearance: {
    type: String,
    default: 'card'
  },
  // 外部可覆盖状态：'running' | 'completed' | 'failed'
  status: {
    type: String,
    default: ''
  },
  // 即使没有 tool_call_result 也展示结果区
  forceShowResult: {
    type: Boolean,
    default: false
  }
})

const slots = useSlots()
const agentStore = useAgentStore()
const availableTools = computed(() => {
  const toolsItem = agentStore.configurableItems?.find((i) => i.kind === 'tools')
  return toolsItem?.options || {}
})

const isTimeline = computed(() => props.appearance === 'timeline')

const elementsProps = computed(() =>
  toElementsToolProps(props.toolCall, { status: props.status })
)

const hasCustomHeader = computed(
  () =>
    !!(
      slots.header ||
      slots['header-success'] ||
      slots['header-error'] ||
      slots['header-running']
    )
)

const isCompleted = computed(
  () => props.toolCall.status === 'success' || !!props.toolCall.tool_call_result
)
const isFailed = computed(() => props.toolCall.status === 'error')

const toolId = computed(() => getToolCallId(props.toolCall))

const toolName = computed(() => {
  const toolsList = availableTools.value ? Object.values(availableTools.value) : []
  const tool = toolsList.find((t) => t.id === toolId.value)
  return tool ? tool.name : toolId.value
})

const parsedInput = computed(() => parseToolCallArgs(props.toolCall) || {})

const formattedArgs = computed(() => {
  const args = props.toolCall.args ? props.toolCall.args : props.toolCall.function?.arguments
  if (!args) return ''
  try {
    if (typeof args === 'string' && args.trim().startsWith('{')) {
      return JSON.stringify(JSON.parse(args), null, 2)
    }
    if (typeof args === 'object' && args !== null) {
      return JSON.stringify(args, null, 2)
    }
  } catch {
    // ignore
  }
  return args
})

const hasParams = computed(() => {
  const argsStr = String(props.toolCall.args || props.toolCall.function?.arguments || '')
  return argsStr.length > 2
})

const resultContent = computed(() => props.toolCall.tool_call_result?.content)
const hasResult = computed(() => !!resultContent.value)

const parsedResultData = computed(() => {
  const content = resultContent.value
  if (typeof content === 'string') {
    try {
      return JSON.parse(content)
    } catch {
      return content
    }
  }
  return content
})
</script>

<style lang="less" scoped>
.tool-call-display {
  margin-bottom: 8px;

  &:last-child {
    margin-bottom: 0;
  }

  &.is-timeline {
    margin-bottom: 0;
  }
}

.tool-header-custom {
  .tool-name {
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  }

  :deep(.sep-header) {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  :deep(.note) {
    color: var(--muted-foreground, #64748b);
    font-weight: 500;
  }

  :deep(.code) {
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    word-break: break-all;
  }
}

.tool-body-block {
  :deep(.params-title),
  :deep(.result-title) {
    display: none;
  }
}

.params-block {
  :deep(> .tool-input),
  :deep(> [class*='space-y-2']) {
    padding-top: 0;
  }
}

.result-block {
  padding: 0 0 8px;
}
</style>
