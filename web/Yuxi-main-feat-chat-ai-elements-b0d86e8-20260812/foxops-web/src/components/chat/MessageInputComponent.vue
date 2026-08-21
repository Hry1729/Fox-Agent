<!-- MessageInputComponent（搬自 Yuxi，改 antd->Element Plus、lucide->离线 SVG、less->scss） -->
<template>
  <div class="input-box" :class="customClasses" @click="focusInput">
    <div class="top-slot">
      <slot name="top"></slot>
    </div>

    <div class="expand-options" v-if="hasOptionsLeft">
      <slot name="options-left"></slot>
      <slot name="actions-left"></slot>
    </div>

    <div
      ref="inputRef"
      class="user-input mention-editor"
      role="textbox"
      aria-multiline="true"
      :aria-label="placeholder"
      :contenteditable="disabled ? 'false' : 'true'"
      :data-placeholder="placeholder"
      @keydown="handleKeyPress"
      @keyup="handleKeyUp"
      @input="handleInput"
      @focus="focusInput"
      @click="handleEditorClick"
      @paste="handlePaste"
      @compositionstart="handleCompositionStart"
      @compositionend="handleCompositionEnd"
    ></div>

    <!-- @ 提及选择弹窗 -->
    <div v-if="mentionPopupVisible" ref="mentionDropdownRef" class="mention-dropdown-wrapper">
      <div class="mention-popup" @mousedown.prevent>
        <!-- 文件列表 -->
        <div v-if="mentionItems.files.length > 0 || showFileSearchPrompt" class="mention-group">
          <div class="mention-group-title">文件</div>
          <div v-if="showFileSearchPrompt" class="mention-search-placeholder">输入相关内容以搜索文件</div>
          <template v-else>
            <div
              v-for="(item, index) in mentionItems.files"
              :key="'file-' + item.value"
              :class="['mention-item', 'file-item', { active: isItemSelected('file', index) }]"
              @click="insertMention(item)"
            >
              <div class="file-info-left">
                <FileTypeIcon :name="item.label" :is-dir="item.is_dir" :size="15" class="file-type-icon" />
                <span class="file-name" :title="item.label">
                  <span v-for="(part, pIdx) in splitTextByQuery(item.label, mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span>
                </span>
              </div>
              <span v-if="formatMentionPath(item.description)" class="file-parent-dir" :title="formatMentionPath(item.description)">
                <span v-for="(part, pIdx) in splitTextByQuery(formatMentionPath(item.description), mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span>
              </span>
            </div>
          </template>
        </div>

        <!-- 知识库 -->
        <div v-if="mentionItems.knowledgeBases.length > 0" class="mention-group">
          <div class="mention-group-title">知识库</div>
          <div v-for="(item, index) in mentionItems.knowledgeBases" :key="'kb-' + item.value" :class="['mention-item', 'resource-item', { active: isItemSelected('knowledge', index) }]" @click="insertMention(item)">
            <div class="resource-name"><span v-for="(part, pIdx) in splitTextByQuery(item.label, mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span></div>
            <div v-if="getMentionDescription(item.description)" class="resource-description" :title="getMentionDescription(item.description)"><span v-for="(part, pIdx) in splitTextByQuery(getMentionDescription(item.description), mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span></div>
          </div>
        </div>

        <!-- MCP -->
        <div v-if="mentionItems.mcps.length > 0" class="mention-group">
          <div class="mention-group-title">MCP</div>
          <div v-for="(item, index) in mentionItems.mcps" :key="'mcp-' + item.value" :class="['mention-item', 'resource-item', { active: isItemSelected('mcp', index) }]" @click="insertMention(item)">
            <div class="resource-name"><span v-for="(part, pIdx) in splitTextByQuery(item.label, mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span></div>
            <div v-if="getMentionDescription(item.description)" class="resource-description" :title="getMentionDescription(item.description)"><span v-for="(part, pIdx) in splitTextByQuery(getMentionDescription(item.description), mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span></div>
          </div>
        </div>

        <!-- Skills -->
        <div v-if="mentionItems.skills.length > 0" class="mention-group">
          <div class="mention-group-title">Skills</div>
          <div v-for="(item, index) in mentionItems.skills" :key="'skill-' + item.value" :class="['mention-item', 'resource-item', { active: isItemSelected('skill', index) }]" @click="insertMention(item)">
            <div class="resource-name"><span v-for="(part, pIdx) in splitTextByQuery(item.label, mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span></div>
            <div v-if="getMentionDescription(item.description)" class="resource-description" :title="getMentionDescription(item.description)"><span v-for="(part, pIdx) in splitTextByQuery(getMentionDescription(item.description), mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span></div>
          </div>
        </div>

        <!-- 子智能体 -->
        <div v-if="mentionItems.subagents.length > 0" class="mention-group">
          <div class="mention-group-title">子智能体</div>
          <div v-for="(item, index) in mentionItems.subagents" :key="'subagent-' + item.value" :class="['mention-item', 'resource-item', { active: isItemSelected('subagent', index) }]" @click="insertMention(item)">
            <div class="resource-name"><span v-for="(part, pIdx) in splitTextByQuery(item.label, mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span></div>
            <div v-if="getMentionDescription(item.description)" class="resource-description" :title="getMentionDescription(item.description)"><span v-for="(part, pIdx) in splitTextByQuery(getMentionDescription(item.description), mentionQuery)" :key="pIdx" :class="{ 'query-match': part.isMatch }">{{ part.text }}</span></div>
          </div>
        </div>

        <div v-if="!hasAnyItems" class="mention-empty">暂无可引用的项</div>
      </div>
    </div>

    <div class="send-button-container">
      <slot name="actions-right"></slot>
      <button
        type="button"
        @click="handleSendOrStop"
        :disabled="sendButtonDisabled"
        class="send-button"
        :title="isLoading ? '停止回答' : '发送'"
      >
        <svg v-if="isLoading" width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>
        <svg v-else width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="19" x2="12" y2="5"/><polyline points="5 12 12 5 19 12"/></svg>
      </button>
    </div>

    <div class="bottom-slot"><slot name="bottom"></slot></div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, nextTick, watch, onBeforeUnmount, useSlots, h, render } from 'vue'
import { searchMentionFiles } from '@/api/mention'
import FileTypeIcon from '@/components/common/FileTypeIcon.vue'
import { getMentionIconComponent, getMentionIconStyle, MENTION_ICON_SIZE, MENTION_ICON_STROKE_WIDTH } from '@/utils/mention_icon_utils'
import {
  buildMentionDisplayLabels, expandMentionDeletionRange, findActiveMentionQuery,
  formatMentionToken, getMentionDisplayLabel, mentionTypePrefixMap,
  parseMentionText, replaceRawRange
} from '@/utils/mention_utils'

const mentionDropdownRef = ref(null)
const closeMentionPopup = (e) => {
  if (!mentionPopupVisible.value) return
  if (inputRef.value?.contains(e.target)) return
  if (mentionDropdownRef.value?.contains(e.target)) return
  mentionPopupVisible.value = false
}

const inputRef = ref(null)
const optionsExpanded = ref(false)
const debounceTimer = ref(null)
const props = defineProps({
  modelValue: { type: String, default: '' },
  placeholder: { type: String, default: '输入问题...' },
  isLoading: { type: Boolean, default: false },
  disabled: { type: Boolean, default: false },
  sendButtonDisabled: { type: Boolean, default: false },
  autoSize: { type: Object, default: () => ({ minRows: 2, maxRows: 6 }) },
  customClasses: { type: Object, default: () => ({}) },
  mention: { type: Object, default: () => null },
  threadId: { type: String, default: '' }
})

const emit = defineEmits(['update:modelValue', 'send', 'keydown'])
const slots = useSlots()

const mentionEnabled = computed(() => !!props.mention)
const mentionDisplayLabels = computed(() => buildMentionDisplayLabels(props.mention || {}))

let lastRawSelectionRange = null
let lastSyncedEditorValue = props.modelValue || ''

const getStoredRawSelectionRange = () => {
  const length = getEditorRawValue().length
  if (!lastRawSelectionRange) return { start: length, end: length, collapsed: true }
  const start = Math.max(0, Math.min(lastRawSelectionRange.start, length))
  const end = Math.max(start, Math.min(lastRawSelectionRange.end, length))
  return { start, end, collapsed: start === end }
}
const rememberRawSelectionRange = (range) => { lastRawSelectionRange = { ...range }; return range }

const formatMentionPath = (path) => {
  if (!path) return ''
  let cleanPath = path.replace(/^\/?home\/gem\/user-data\/?/, '')
  if (cleanPath.startsWith('/')) cleanPath = cleanPath.substring(1)
  let isDir = cleanPath.endsWith('/')
  let pathForParent = isDir ? cleanPath.substring(0, cleanPath.length - 1) : cleanPath
  const lastSlashIndex = pathForParent.lastIndexOf('/')
  if (lastSlashIndex === -1) return ''
  return pathForParent.substring(0, lastSlashIndex + 1)
}

const getMentionDescription = (description) => {
  const value = String(description || '').trim()
  if (!value || value === '暂无描述') return ''
  return value
}

const splitTextByQuery = (text, query) => {
  if (!text) return []
  if (!query) return [{ text, isMatch: false }]
  const escapedQuery = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const regex = new RegExp(`(${escapedQuery})`, 'gi')
  const parts = text.split(regex)
  return parts.map((part) => ({ text: part, isMatch: part.toLowerCase() === query.toLowerCase() }))
}

// === contenteditable 核心逻辑（纯 DOM，原样搬） ===
const isTextNode = (node) => node?.nodeType === Node.TEXT_NODE
const isElementNode = (node) => node?.nodeType === Node.ELEMENT_NODE
const isMentionNode = (node) => isElementNode(node) && node.dataset?.mentionRaw !== undefined
const isLineBreakNode = (node) => isElementNode(node) && node.tagName === 'BR'
const childIndex = (node) => Array.prototype.indexOf.call(node.parentNode?.childNodes || [], node)

const getRawNodeLength = (node) => {
  if (!node) return 0
  if (isTextNode(node)) return node.textContent?.length || 0
  if (isMentionNode(node)) return node.dataset.mentionRaw?.length || 0
  if (isLineBreakNode(node)) return 1
  return Array.from(node.childNodes || []).reduce((total, child) => total + getRawNodeLength(child), 0)
}

const serializeEditorNode = (node) => {
  if (!node) return ''
  if (isTextNode(node)) return node.textContent || ''
  if (isMentionNode(node)) return node.dataset.mentionRaw || ''
  if (isLineBreakNode(node)) return '\n'
  return Array.from(node.childNodes || []).map((child) => serializeEditorNode(child)).join('')
}

const serializeEditorContent = () => serializeEditorNode(inputRef.value)
const getEditorRawValue = () => (inputRef.value ? serializeEditorContent() : inputValue.value)

const unmountEditorMentionIcons = () => {
  inputRef.value?.querySelectorAll('.mention-ref-icon[data-vue-icon]').forEach((container) => render(null, container))
}

const createEditorMentionElement = (segment) => {
  const token = document.createElement('span')
  token.className = `mention-ref-token mention-ref-${segment.type} mention-ref-editable`
  token.contentEditable = 'false'
  token.dataset.mentionRaw = segment.raw
  token.dataset.mentionType = segment.type
  token.dataset.mentionValue = segment.value
  token.title = segment.raw

  const icon = document.createElement('span')
  icon.className = 'mention-ref-icon'
  icon.dataset.vueIcon = 'true'
  if (segment.type === 'file') {
    render(h(FileTypeIcon, { name: segment.value, isDir: segment.value.endsWith('/'), size: MENTION_ICON_SIZE }), icon)
  } else {
    render(h(getMentionIconComponent(segment.type), { size: MENTION_ICON_SIZE, strokeWidth: MENTION_ICON_STROKE_WIDTH }), icon)
  }
  token.appendChild(icon)

  const label = document.createElement('span')
  label.className = 'mention-ref-label'
  label.textContent = getMentionDisplayLabel(segment.type, segment.value, mentionDisplayLabels.value)
  token.appendChild(label)
  return token
}

const renderEditorContent = (raw = '') => {
  const editor = inputRef.value
  if (!editor) return
  unmountEditorMentionIcons()
  editor.replaceChildren()
  parseMentionText(raw).forEach((segment) => {
    editor.appendChild(segment.kind === 'text' ? document.createTextNode(segment.text) : createEditorMentionElement(segment))
  })
  lastSyncedEditorValue = String(raw || '')
}

const isNodeInEditor = (node) => {
  const editor = inputRef.value
  if (!editor || !node) return false
  const element = isElementNode(node) ? node : node.parentNode
  return element === editor || editor.contains(element)
}

const getRawOffsetFromDomPoint = (container, offset) => {
  const editor = inputRef.value
  if (!editor || !isNodeInEditor(container)) return getEditorRawValue().length
  let rawOffset = 0
  let found = false
  const visit = (node) => {
    if (!node || found) return
    if (node === container) {
      if (isTextNode(node)) { rawOffset += Math.min(offset, node.textContent?.length || 0) }
      else if (isMentionNode(node)) { rawOffset += offset > 0 ? getRawNodeLength(node) : 0 }
      else { const children = Array.from(node.childNodes || []); for (let i = 0; i < Math.min(offset, children.length); i++) rawOffset += getRawNodeLength(children[i]) }
      found = true; return
    }
    if (isTextNode(node) || isMentionNode(node) || isLineBreakNode(node)) { rawOffset += getRawNodeLength(node); return }
    for (const child of Array.from(node.childNodes || [])) { visit(child); if (found) return }
  }
  visit(editor)
  return rawOffset
}

const getRawSelectionRange = () => {
  const selection = window.getSelection()
  if (!selection || selection.rangeCount === 0 || !isNodeInEditor(selection.anchorNode)) return getStoredRawSelectionRange()
  const anchor = getRawOffsetFromDomPoint(selection.anchorNode, selection.anchorOffset)
  const focus = getRawOffsetFromDomPoint(selection.focusNode, selection.focusOffset)
  return rememberRawSelectionRange({ start: Math.min(anchor, focus), end: Math.max(anchor, focus), collapsed: anchor === focus })
}

const getDomPointForRawOffset = (rawOffset) => {
  const editor = inputRef.value
  const offset = Math.max(0, Math.min(rawOffset, getEditorRawValue().length))
  let remaining = offset
  const pointBeforeNode = (node) => ({ node: node.parentNode || editor, offset: childIndex(node) })
  const pointAfterNode = (node) => ({ node: node.parentNode || editor, offset: childIndex(node) + 1 })
  const visit = (node) => {
    if (!node) return null
    if (isTextNode(node)) { const length = node.textContent?.length || 0; if (remaining <= length) return { node, offset: remaining }; remaining -= length; return null }
    if (isMentionNode(node) || isLineBreakNode(node)) { const length = getRawNodeLength(node); if (remaining === 0) return pointBeforeNode(node); if (remaining <= length) return pointAfterNode(node); remaining -= length; return null }
    for (const child of Array.from(node.childNodes || [])) { const point = visit(child); if (point) return point }
    return node === editor ? { node: editor, offset: editor.childNodes.length } : null
  }
  return visit(editor) || { node: editor, offset: editor?.childNodes.length || 0 }
}

const restoreEditorSelection = (start, end = start) => {
  const editor = inputRef.value
  const selection = window.getSelection()
  if (!editor || !selection) return
  const startPoint = getDomPointForRawOffset(start)
  const endPoint = getDomPointForRawOffset(end)
  const range = document.createRange()
  range.setStart(startPoint.node, startPoint.offset)
  range.setEnd(endPoint.node, endPoint.offset)
  selection.removeAllRanges()
  selection.addRange(range)
  rememberRawSelectionRange({ start, end, collapsed: start === end })
  editor.focus()
}

const updateRawValue = (value, caretStart, caretEnd = caretStart) => {
  renderEditorContent(value)
  emit('update:modelValue', value)
  nextTick(() => { restoreEditorSelection(caretStart, caretEnd); adjustTextareaHeight(); if (mentionEnabled.value) checkMentionTrigger() })
}

const replaceCurrentRawSelection = (replacement) => {
  const currentValue = getEditorRawValue()
  const range = getRawSelectionRange()
  const nextValue = replaceRawRange(currentValue, range.start, range.end, replacement)
  const nextOffset = range.start + replacement.length
  updateRawValue(nextValue, nextOffset)
}

// === @ 提及逻辑（原样搬） ===
const mentionPopupVisible = ref(false)
const mentionQuery = ref('')
const mentionItems = ref({ files: [], knowledgeBases: [], mcps: [], skills: [], subagents: [] })
const mentionSelectedIndex = ref(0)
const searchRequestId = ref(0)
const isComposing = ref(false)
let activeAbortController = null
let mentionSearchTimer = null

const checkMentionTrigger = () => {
  if (!inputRef.value || !mentionEnabled.value) return false
  const selectionRange = getRawSelectionRange()
  if (!selectionRange.collapsed) { mentionPopupVisible.value = false; return false }
  const activeMention = findActiveMentionQuery(getEditorRawValue(), selectionRange.end)
  if (activeMention) {
    const sameQuery = mentionPopupVisible.value && mentionQuery.value === activeMention.query
    mentionQuery.value = activeMention.query
    mentionPopupVisible.value = true
    if (!sameQuery) { mentionSelectedIndex.value = 0; updateMentionItems(mentionQuery.value) }
    return true
  }
  mentionPopupVisible.value = false
  return false
}

const updateMentionItems = (query = '') => {
  const normalizedQuery = String(query || '')
  if (!normalizedQuery) { clearTimeout(mentionSearchTimer); if (activeAbortController) { activeAbortController.abort(); activeAbortController = null }; searchRequestId.value++ }
  if (!props.mention) { mentionItems.value = { files: [], knowledgeBases: [], mcps: [], skills: [], subagents: [] }; return }
  const lowerQuery = normalizedQuery.toLowerCase()
  const { files = [], knowledgeBases = [], mcps = [], skills = [], subagents = [] } = props.mention
  const filterItems = (list) => list.filter((item) => {
    const searchTexts = [item.label, item.value, item.description, item.resourceId, item.tokenLabel, item.type, mentionTypePrefixMap[item.type]]
    return searchTexts.some((text) => String(text || '').toLowerCase().includes(lowerQuery))
  })
  const localFileItems = files.map((f) => { const path = f.path || ''; const fileName = path.split('/').pop() || path; return { value: path, label: fileName, type: 'file', insertValue: path || fileName, tokenLabel: formatMentionToken('file', fileName), description: path } })
  const filteredLocalFiles = normalizedQuery ? filterItems(localFileItems) : []
  const knowledgeItems = knowledgeBases.map((kb) => { const kbName = kb.name || ''; return { value: kbName, label: kbName, type: 'knowledge', insertValue: kbName, tokenLabel: formatMentionToken('knowledge', kbName), description: kb.description || '', resourceId: kb.kb_id } })
  const mcpItems = mcps.map((m) => { const v = m.slug || m.value || m.id || m.name || ''; const l = m.name || m.label || v; return { value: v, label: l, type: 'mcp', insertValue: v, tokenLabel: formatMentionToken('mcp', l), description: m.description || '' } })
  const skillItems = skills.map((s) => { const v = s.slug || s.value || s.id || s.name || ''; const l = s.name || s.label || v; return { value: v, label: l, type: 'skill', insertValue: v, tokenLabel: formatMentionToken('skill', l), description: s.description || '' } })
  const subagentItems = subagents.map((sa) => { const v = sa.id || sa.value || sa.slug || sa.name || ''; const l = sa.name || sa.label || v; return { value: v, label: l, type: 'subagent', insertValue: v, tokenLabel: formatMentionToken('subagent', l), description: sa.description || '' } })

  mentionItems.value = { files: filteredLocalFiles, knowledgeBases: filterItems(knowledgeItems), mcps: filterItems(mcpItems), skills: filterItems(skillItems), subagents: filterItems(subagentItems) }

  if (normalizedQuery) {
    const activeThreadId = props.threadId || ''
    clearTimeout(mentionSearchTimer)
    if (activeAbortController) { activeAbortController.abort(); activeAbortController = null }
    searchRequestId.value++
    const currentId = searchRequestId.value
    mentionSearchTimer = setTimeout(async () => {
      activeAbortController = new AbortController()
      try {
        const responseData = await searchMentionFiles(activeThreadId, normalizedQuery, activeAbortController.signal)
        if (currentId === searchRequestId.value && Array.isArray(responseData)) {
          const remoteFileItems = responseData.map((f) => { const path = f.path || ''; const fileName = f.name || path.split('/').pop() || path; return { value: path, label: fileName, type: 'file', insertValue: path || fileName, tokenLabel: formatMentionToken('file', fileName), description: path, is_dir: f.is_dir, source: f.source } })
          const seenValues = new Set(filteredLocalFiles.map((x) => x.value))
          const mergedFiles = [...filteredLocalFiles]
          remoteFileItems.forEach((item) => { if (!seenValues.has(item.value)) { seenValues.add(item.value); mergedFiles.push(item) } })
          mentionItems.value.files = mergedFiles
        }
      } catch (error) { if (error.name !== 'AbortError') console.error('Mention search error:', error) }
      finally { if (currentId === searchRequestId.value) activeAbortController = null }
    }, 250)
  }
}

const isItemSelected = (type, index) => {
  if (mentionSelectedIndex.value < 0) return false
  const f = mentionItems.value.files.length, kb = mentionItems.value.knowledgeBases.length, m = mentionItems.value.mcps.length, s = mentionItems.value.skills.length
  if (type === 'file') return mentionSelectedIndex.value === index
  if (type === 'knowledge') return mentionSelectedIndex.value === f + index
  if (type === 'mcp') return mentionSelectedIndex.value === f + kb + index
  if (type === 'skill') return mentionSelectedIndex.value === f + kb + m + index
  return mentionSelectedIndex.value === f + kb + m + s + index
}

const showFileSearchPrompt = computed(() => Boolean(props.mention?.files?.length) && !mentionQuery.value)
const hasAnyItems = computed(() => {
  const items = mentionItems.value
  return showFileSearchPrompt.value || items.files.length > 0 || items.knowledgeBases.length > 0 || items.mcps.length > 0 || items.skills.length > 0 || items.subagents.length > 0
})

const insertMention = (item) => {
  if (!inputRef.value) return
  const currentValue = getEditorRawValue()
  const selectionRange = getRawSelectionRange()
  const activeMention = findActiveMentionQuery(currentValue, selectionRange.end)
  if (!activeMention) return
  const mentionValue = item.insertValue || item.value
  const mentionText = `${formatMentionToken(item.type, mentionValue)} `
  const newValue = replaceRawRange(currentValue, activeMention.start, activeMention.end, mentionText)
  const newCursorPos = activeMention.start + mentionText.length
  mentionPopupVisible.value = false
  mentionQuery.value = ''
  updateRawValue(newValue, newCursorPos)
}

const scrollToItem = (index) => {
  nextTick(() => {
    const popup = mentionDropdownRef.value?.querySelector('.mention-popup')
    if (!popup) return
    const items = popup.querySelectorAll('.mention-item')
    const selectedItem = items[index]
    if (selectedItem) {
      const popupRect = popup.getBoundingClientRect()
      const itemRect = selectedItem.getBoundingClientRect()
      if (itemRect.bottom > popupRect.bottom || itemRect.top < popupRect.top) selectedItem.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
    }
  })
}

const handleMentionNavigation = (e) => {
  if (!mentionPopupVisible.value) return
  const allItems = [...mentionItems.value.files, ...mentionItems.value.knowledgeBases, ...mentionItems.value.mcps, ...mentionItems.value.skills, ...mentionItems.value.subagents]
  const total = allItems.length
  if (total === 0) return
  if (e.key === 'ArrowDown') { e.preventDefault(); mentionSelectedIndex.value = (mentionSelectedIndex.value + 1) % total; scrollToItem(mentionSelectedIndex.value) }
  else if (e.key === 'ArrowUp') { e.preventDefault(); mentionSelectedIndex.value = (mentionSelectedIndex.value - 1 + total) % total; scrollToItem(mentionSelectedIndex.value) }
  else if (e.key === 'Enter' || e.key === 'Tab') { if (mentionSelectedIndex.value >= 0 && mentionSelectedIndex.value < total) { e.preventDefault(); insertMention(allItems[mentionSelectedIndex.value]) } }
  else if (e.key === 'Escape') { e.preventDefault(); mentionPopupVisible.value = false }
}

const hasOptionsLeft = computed(() => { const slot = slots['options-left']; if (!slot) return false; const renderedNodes = slot(); return Boolean(renderedNodes && renderedNodes.length) })
const inputValue = computed({ get: () => props.modelValue, set: (val) => emit('update:modelValue', val) })

const handleMentionDeletion = (e) => {
  if (e.key !== 'Backspace' && e.key !== 'Delete') return false
  const currentValue = getEditorRawValue()
  const selectionRange = getRawSelectionRange()
  const expandedRange = expandMentionDeletionRange(currentValue, selectionRange.start, selectionRange.end, e.key === 'Delete' ? 'forward' : 'backward')
  if (!expandedRange) return false
  e.preventDefault()
  const nextValue = replaceRawRange(currentValue, expandedRange.start, expandedRange.end, '')
  mentionPopupVisible.value = false
  updateRawValue(nextValue, expandedRange.start)
  return true
}

const handleKeyPress = (e) => {
  if (mentionPopupVisible.value) { if (['ArrowDown', 'ArrowUp', 'Enter', 'Tab', 'Escape'].includes(e.key)) { handleMentionNavigation(e); return } }
  if (handleMentionDeletion(e)) return
  if (e.key === 'Enter' && e.shiftKey) { e.preventDefault(); replaceCurrentRawSelection('\n'); return }
  emit('keydown', e)
}

const shouldCheckMentionOnKeyUp = (e) => { if (!e) return false; if (e.key.length === 1) return true; return e.key === 'Backspace' || e.key === 'Delete' }
const handleKeyUp = (e) => { if (!mentionEnabled.value || isComposing.value || !shouldCheckMentionOnKeyUp(e)) return; nextTick(() => { checkMentionTrigger() }) }
const handleInput = () => {
  if (isComposing.value) return
  if (inputRef.value && !inputRef.value.querySelector('.mention-ref-token')) { const text = inputRef.value.textContent || ''; if (!text.trim()) inputRef.value.replaceChildren() }
  const value = serializeEditorContent()
  lastSyncedEditorValue = value
  emit('update:modelValue', value)
  adjustTextareaHeight()
  if (mentionEnabled.value) nextTick(() => { checkMentionTrigger() })
}
const handlePaste = (e) => { e.preventDefault(); const text = e.clipboardData?.getData('text/plain') || ''; replaceCurrentRawSelection(text) }
const handleCompositionStart = () => { isComposing.value = true }
const handleCompositionEnd = () => { isComposing.value = false; handleInput() }
const handleSendOrStop = () => { emit('send') }

const adjustTextareaHeight = () => { if (!inputRef.value) return; const textarea = inputRef.value; textarea.style.height = 'auto'; textarea.style.height = `${Math.min(textarea.scrollHeight, 200)}px` }
const focusInput = () => { if (inputRef.value && !props.disabled) { inputRef.value.focus(); if (mentionEnabled.value) nextTick(() => { checkMentionTrigger() }) } }
const handleEditorClick = () => { if (mentionEnabled.value) nextTick(() => { checkMentionTrigger() }) }

watch(inputValue, (value) => {
  if (value !== lastSyncedEditorValue) renderEditorContent(value || '')
  if (debounceTimer.value) clearTimeout(debounceTimer.value)
  debounceTimer.value = setTimeout(() => { nextTick(() => { adjustTextareaHeight() }) }, 100)
})

onMounted(() => {
  document.addEventListener('click', closeMentionPopup)
  nextTick(() => { if (inputRef.value) { renderEditorContent(inputValue.value || ''); adjustTextareaHeight(); inputRef.value.focus() } })
})

onBeforeUnmount(() => {
  unmountEditorMentionIcons()
  if (debounceTimer.value) clearTimeout(debounceTimer.value)
  if (mentionSearchTimer) clearTimeout(mentionSearchTimer)
  if (activeAbortController) activeAbortController.abort()
  document.removeEventListener('click', closeMentionPopup)
})

defineExpose({ focus: () => inputRef.value?.focus(), closeOptions: () => { optionsExpanded.value = false } })
</script>

<style scoped>
.input-box {
  display: grid;
  width: 100%;
  margin: 0 auto;
  border: 1px solid var(--el-border-color, #dcdfe6);
  border-radius: 12px;
  background: var(--el-bg-color, #fff);
  gap: 0px;
  position: relative;
  padding: 12px 12px 10px 12px;
  grid-template-columns: auto 1fr;
  grid-template-rows: auto auto auto;
  grid-template-areas: 'top top' 'input input' 'options send';
}
.top-slot { display: flex; grid-area: top; }
.expand-options { grid-area: options; justify-self: start; display: flex; align-items: center; gap: 8px; }
.user-input {
  grid-area: input;
  width: 100%;
  padding: 0;
  background-color: transparent;
  border: none;
  margin: 0 0 8px 0;
  color: var(--el-text-color-primary, #303133);
  font-size: 14px;
  outline: none;
  resize: none;
  line-height: 1.5;
  font-family: inherit;
  min-height: 44px;
  max-height: 200px;
  overflow-y: auto;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  cursor: text;
}
.user-input:focus { outline: none; box-shadow: none; }
.user-input.mention-editor { position: relative; }
.user-input.mention-editor:empty::before {
  content: attr(data-placeholder);
  position: absolute; left: 0; top: 0;
  color: var(--el-text-color-placeholder, #a8abb2);
  pointer-events: none;
}
.user-input[contenteditable='false'] { cursor: not-allowed; }
.user-input :deep(.mention-ref-token) {
  display: inline-flex; align-items: baseline; gap: 2px;
  max-width: min(100%, 360px);
  color: var(--el-color-primary, #409eff);
  line-height: normal; vertical-align: baseline;
  white-space: nowrap; user-select: all;
}
.user-input :deep(.mention-ref-icon) {
  position: relative; top: 2px;
  display: inline-flex; align-items: center;
  flex-shrink: 0; font-size: 13px; line-height: 1; margin-left: 4px;
}
.user-input :deep(.mention-ref-icon svg) { display: block; }
.user-input :deep(.mention-ref-label) {
  min-width: 0; overflow: hidden; text-overflow: ellipsis;
  line-height: normal; font-weight: 500;
}
.send-button-container { grid-area: send; justify-self: end; display: flex; align-items: center; justify-content: center; gap: 8px; }
.send-button {
  height: 32px; width: 32px; cursor: pointer;
  background-color: var(--el-color-primary, #409eff);
  border-radius: 50%; border: none;
  transition: all 0.2s ease;
  color: #fff; padding: 0;
  display: flex; align-items: center; justify-content: center;
  flex-shrink: 0;
  box-sizing: border-box;
}
.send-button:hover { opacity: 0.9; }
.send-button:disabled { opacity: 0.5; cursor: not-allowed; }

/* @ 提及弹窗 */
.mention-dropdown-wrapper { position: absolute; bottom: 100%; left: 0; right: 0; margin-bottom: 8px; z-index: 1000; }
.mention-popup {
  width: 100%; max-height: 280px; overflow-y: auto;
  background: var(--el-bg-color, #fff);
  border-radius: 8px;
  box-shadow: 0 -4px 16px rgba(0,0,0,0.08), 0 4px 16px rgba(0,0,0,0.12);
  border: 1px solid var(--el-border-color-light, #e4e7ed);
  padding: 8px 0;
}
.mention-popup .mention-group { margin-bottom: 4px; }
.mention-popup .mention-group:last-child { margin-bottom: 0; }
.mention-popup .mention-group-title {
  font-size: 12px; color: var(--el-text-color-secondary, #909399);
  padding: 4px 8px; display: flex; align-items: center; gap: 4px;
  border-bottom: 1px solid var(--el-border-color-lighter, #ebeef5);
  margin-bottom: 2px;
}
.mention-popup .mention-item {
  padding: 4px 8px; cursor: pointer; font-size: 13px;
  color: var(--el-text-color-regular, #606266);
  transition: all 0.15s ease; margin: 1px 4px; border-radius: 4px;
}
.mention-popup .mention-item.resource-item {
  display: flex; align-items: center; gap: 8px; min-width: 0; padding: 6px 10px;
}
.mention-popup .mention-item.resource-item .resource-name { color: var(--el-text-color-primary, #303133); font-weight: 500; flex: 0 0 auto; white-space: nowrap; }
.mention-popup .mention-item.resource-item .resource-description { color: var(--el-text-color-secondary, #909399); font-size: 12px; line-height: 1.35; flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mention-popup .mention-item.file-item { display: flex; flex-direction: row; align-items: center; gap: 0; padding: 6px 10px; }
.mention-popup .mention-item.file-item .file-info-left { display: flex; align-items: center; gap: 8px; flex-shrink: 1; min-width: 0; }
.mention-popup .mention-item.file-item .file-info-left .file-type-icon { font-size: 15px; flex-shrink: 0; display: flex; align-items: center; }
.mention-popup .mention-item.file-item .file-info-left .file-name { font-weight: 500; color: var(--el-text-color-primary, #303133); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
.mention-popup .mention-item.file-item .file-parent-dir { font-size: 11px; color: var(--el-text-color-placeholder, #a8abb2); margin-left: 8px; flex-shrink: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mention-popup .mention-item:hover { background-color: var(--el-color-primary-light-9, #ecf5ff); }
.mention-popup .mention-item:hover.resource-item .resource-name { color: var(--el-color-primary, #409eff); }
.mention-popup .mention-item.active { background-color: var(--el-fill-color-light, #f5f7fa); }
.mention-popup .mention-item.active.resource-item .resource-name { color: var(--el-color-primary, #409eff); }
.mention-popup .query-match { color: #fa8c16; font-weight: 700; }
.mention-popup .mention-empty { text-align: center; padding: 12px 8px; color: var(--el-text-color-placeholder, #a8abb2); font-size: 13px; }
.mention-popup .mention-search-placeholder { padding: 4px 8px; color: var(--el-text-color-placeholder, #a8abb2); font-size: 13px; }
</style>
