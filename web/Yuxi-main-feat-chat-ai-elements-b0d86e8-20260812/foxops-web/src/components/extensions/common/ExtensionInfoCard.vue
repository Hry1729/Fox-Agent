<template>
  <div
    class="extension-info-card"
    :class="{ 'is-disabled': disabled }"
    :data-accent="accent"
    role="button"
    :tabindex="disabled ? -1 : 0"
    @click="onClick"
    @keydown.enter.prevent="onClick"
    @keydown.space.prevent="onClick"
  >
    <div class="card-shell">
      <div class="card-header">
        <div class="card-icon">
          <img v-if="brandIconUrl" :src="brandIconUrl" alt="" class="brand-icon" />
          <slot v-else name="icon">
            <ArtSvgIcon :icon="icon" />
          </slot>
        </div>
        <div class="card-titles">
          <h3 class="card-title" :title="title">{{ title }}</h3>
          <p v-if="subtitle" class="card-subtitle" :title="subtitle">{{ subtitle }}</p>
        </div>
        <div class="card-action" @click="onActionClick">
          <slot name="action">
            <ArtSvgIcon v-if="showArrow" icon="ri:arrow-right-s-line" class="card-arrow" />
          </slot>
        </div>
      </div>

      <p v-if="!$slots.description" class="card-description" :title="normalizedDescription">
        {{ normalizedDescription }}
      </p>
      <slot v-else name="description" />

      <div v-if="normalizedTags.length || $slots.footer" class="card-bottom">
        <div v-if="normalizedTags.length" class="card-tags">
          <span
            v-for="tag in normalizedTags"
            :key="tag.name"
            class="card-tag"
            :class="resolveTagClass(tag)"
            :title="tag.name"
          >
            {{ tag.name }}
          </span>
        </div>
        <div v-if="$slots.footer" class="card-footer" @click.stop>
          <slot name="footer" />
        </div>
      </div>

      <div v-if="$slots.toolbar" class="card-toolbar" @click.stop>
        <slot name="toolbar" />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
  export type ExtensionCardTag = {
    name: string
    type?: 'primary' | 'success' | 'warning' | 'info' | 'danger'
    color?: 'blue' | 'purple' | 'green' | 'orange' | 'cyan' | 'red' | 'gold'
    variant?: 'type' | 'embed' | 'share' | 'success' | 'warning' | 'danger' | 'neutral'
  }

  type TagInput = string | ExtensionCardTag

  type CardAccent = 'blue' | 'gold' | 'purple' | 'cyan'

  const props = withDefaults(
    defineProps<{
      title: string
      subtitle?: string
      description?: string
      icon?: string
      brandIconUrl?: string
      accent?: CardAccent
      tags?: TagInput[]
      showArrow?: boolean
      disabled?: boolean
    }>(),
    {
      subtitle: '',
      description: '暂无描述',
      icon: 'ri:puzzle-line',
      brandIconUrl: '',
      accent: 'blue',
      tags: () => [],
      showArrow: false,
      disabled: false
    }
  )

  const emit = defineEmits<{ click: [] }>()
  const slots = useSlots()

  const normalizedDescription = computed(() => String(props.description || '').trim() || '暂无描述')

  const normalizedTags = computed(() =>
    (props.tags || []).map((tag) =>
      typeof tag === 'string' ? { name: tag, color: 'blue' as const } : { ...tag }
    )
  )

  const resolveTagClass = (tag: ExtensionCardTag) => {
    if (tag.variant) return `tag-${tag.variant}`
    if (tag.color) return `tag-${tag.color}`
    if (tag.type) return `tag-type-${tag.type}`
    return 'tag-blue'
  }

  const onClick = () => {
    if (props.disabled) return
    emit('click')
  }

  const onActionClick = (event: MouseEvent) => {
    event.stopPropagation()
    // 自定义操作槽自行处理；默认箭头应与整张卡片一致地进入详情页。
    if (!slots.action) onClick()
  }
</script>

<style scoped>
  .extension-info-card {
    display: block;
    width: 100%;
    padding: 0;
    border: none;
    background: transparent;
    text-align: left;
    cursor: pointer;
  }

  .extension-info-card.is-disabled {
    opacity: 0.75;
    cursor: default;
  }

  .card-shell {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 128px;
    padding: 12px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 12px;
    background: linear-gradient(
      to left,
      var(--card-tint) 0%,
      color-mix(in srgb, var(--card-tint) 42%, var(--el-bg-color)) 46%,
      var(--el-bg-color) 100%
    );
    overflow: hidden;
    transition:
      border-color 0.18s ease,
      box-shadow 0.18s ease,
      transform 0.18s ease;
  }

  .extension-info-card:not(.is-disabled):hover .card-shell {
    border-color: var(--card-accent-soft);
    box-shadow: 0 10px 24px color-mix(in srgb, var(--card-accent) 14%, transparent);
    transform: translateY(-1px);
  }

  .extension-info-card:not(.is-disabled):hover .card-arrow {
    opacity: 1;
    transform: translateX(2px);
    color: var(--card-accent);
  }

  .extension-info-card[data-accent='blue'] {
    --card-accent: var(--el-color-primary);
    --card-accent-soft: var(--el-color-primary-light-5);
    --card-tint: color-mix(in srgb, var(--el-color-primary-light-9) 82%, var(--el-bg-color));
  }

  .extension-info-card[data-accent='gold'] {
    --card-accent: var(--el-color-warning);
    --card-accent-soft: var(--el-color-warning-light-5);
    --card-tint: color-mix(in srgb, var(--el-color-warning-light-9) 78%, var(--el-bg-color));
  }

  .extension-info-card[data-accent='purple'] {
    --card-accent: #8b5cf6;
    --card-accent-soft: #c4b5fd;
    --card-tint: color-mix(in srgb, #8b5cf6 11%, var(--el-bg-color));
  }

  .extension-info-card[data-accent='cyan'] {
    --card-accent: #0891b2;
    --card-accent-soft: #67e8f9;
    --card-tint: color-mix(in srgb, #0891b2 11%, var(--el-bg-color));
  }

  .card-header {
    display: flex;
    align-items: flex-start;
    gap: 12px;
  }

  .card-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 38px;
    height: 38px;
    border-radius: 10px;
    border: 1px solid color-mix(in srgb, var(--card-accent) 18%, var(--el-border-color-lighter));
    background: color-mix(in srgb, var(--card-tint) 70%, var(--el-bg-color));
    color: var(--card-accent);
    font-size: 20px;
    flex-shrink: 0;
  }

  .brand-icon {
    width: 22px;
    height: 22px;
    object-fit: contain;
  }

  .card-titles {
    flex: 1;
    min-width: 0;
  }

  .card-title {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    line-height: 1.35;
    color: var(--el-text-color-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-subtitle {
    margin: 4px 0 0;
    font-size: 12px;
    line-height: 1.4;
    color: var(--el-text-color-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-action {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    min-height: 20px;
  }

  .card-arrow {
    font-size: 18px;
    color: var(--el-text-color-placeholder);
    opacity: 0;
    transform: translateX(-2px);
    transition:
      opacity 0.18s ease,
      transform 0.18s ease,
      color 0.18s ease;
  }

  .card-description {
    margin: 0;
    min-height: calc(1.4em * 2);
    font-size: 12px;
    line-height: 1.4;
    color: var(--el-text-color-regular);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .card-bottom {
    margin-top: auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-height: 22px;
  }

  .card-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }

  .card-tag {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    height: 22px;
    padding: 0 8px;
    border-radius: 999px;
    font-size: 11px;
    font-weight: 600;
    line-height: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tag-type {
    background: color-mix(in srgb, var(--card-accent) 12%, var(--el-bg-color));
    color: var(--card-accent);
    border: 1px solid color-mix(in srgb, var(--card-accent) 22%, transparent);
  }

  .tag-blue,
  .tag-type-info,
  .tag-type-primary {
    background: color-mix(in srgb, var(--el-color-primary) 12%, var(--el-bg-color));
    color: var(--el-color-primary);
    border: 1px solid color-mix(in srgb, var(--el-color-primary) 22%, transparent);
  }

  .tag-purple,
  .tag-cyan {
    background: color-mix(in srgb, #8b5cf6 12%, var(--el-bg-color));
    color: #7c3aed;
    border: 1px solid color-mix(in srgb, #8b5cf6 22%, transparent);
  }

  .tag-cyan {
    background: color-mix(in srgb, #0891b2 12%, var(--el-bg-color));
    color: #0891b2;
    border: 1px solid color-mix(in srgb, #0891b2 22%, transparent);
  }

  .tag-green,
  .tag-share,
  .tag-success,
  .tag-type-success {
    background: var(--el-color-success-light-9);
    color: var(--el-color-success);
    border: 1px solid var(--el-color-success-light-7);
  }

  .tag-orange,
  .tag-gold,
  .tag-warning,
  .tag-type-warning {
    background: var(--el-color-warning-light-9);
    color: var(--el-color-warning);
    border: 1px solid var(--el-color-warning-light-7);
  }

  .tag-red,
  .tag-danger,
  .tag-type-danger {
    background: var(--el-color-danger-light-9);
    color: var(--el-color-danger);
    border: 1px solid var(--el-color-danger-light-7);
  }

  .tag-embed,
  .tag-neutral {
    background: var(--el-fill-color-light);
    color: var(--el-text-color-regular);
    border: 1px solid var(--el-border-color-lighter);
  }

  .card-footer {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    flex-shrink: 0;
  }

  .card-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 0;
    padding-top: 2px;
    padding-bottom: 0;
    border-top: 1px solid var(--el-border-color-lighter);
  }
</style>
