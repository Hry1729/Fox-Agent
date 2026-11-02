// 输入框里资料库 chip 的显示名称（UI 1104-06）。
//
// 以前的顺序是「会话里记的名称 → 资料库列表 → 旧绑定名称 → id」。问题有两处：
// 1. 选资料库时前端在拿不到名称的情况下会把 id 当名称写进会话（Host 也会在名称为空时回填 id），
//    这个「名称」排在第一位，资料库列表里明明有真名也被它盖住；
// 2. 已保存会话的本机资料库引用不带名称（旧绑定只覆盖远程资料库），本机资料库列表又只在打开选择器时才读，
//    最后落到 id，于是 chip 上显示的是 94d5990a-c491-46… 这样的 UUID。
// 现在先用实时列表里的名称，记下的名称只有在不等于 id 时才用，都没有时显示「资料库」，不把 id 露给用户。
import type { KnowledgeBindingRecord, KnowledgeReference } from '@/features/conversations/model/types'
import { knowledgeReferenceFromLegacyBinding, knowledgeReferenceKey } from '@/features/conversations/api/desktop-client'

export const KNOWLEDGE_CHIP_FALLBACK_NAME = '资料库'

function usableName(name: string | null | undefined, id: string) {
  const trimmed = name?.trim()
  return trimmed && trimmed !== id ? trimmed : null
}

export function knowledgeChipName(reference: KnowledgeReference, sources: {
  names?: Record<string, string>
  localKnowledgeBases?: ReadonlyArray<{ id: string; name: string }>
  remoteKnowledgeBases?: ReadonlyArray<{ id: string; name: string }>
  bindings?: ReadonlyArray<KnowledgeBindingRecord>
}) {
  const key = knowledgeReferenceKey(reference)
  const listed = reference.source === 'local'
    ? sources.localKnowledgeBases?.find((item) => item.id === reference.id)?.name
    : sources.remoteKnowledgeBases?.find((item) => item.id === reference.id)?.name
  const bound = sources.bindings?.find((binding) => knowledgeReferenceKey(knowledgeReferenceFromLegacyBinding(binding)) === key)?.knowledgeBaseName
  return usableName(listed, reference.id)
    ?? usableName(sources.names?.[key], reference.id)
    ?? usableName(bound, reference.id)
    ?? KNOWLEDGE_CHIP_FALLBACK_NAME
}

/** 引用里有本机资料库、但本机列表里还找不到它时，需要读一次本机资料库列表来拿名称。 */
export function needsLocalKnowledgeNames(references: ReadonlyArray<KnowledgeReference> | undefined, localKnowledgeBases: ReadonlyArray<{ id: string }>) {
  return (references ?? []).some((reference) => reference.source === 'local' && !localKnowledgeBases.some((item) => item.id === reference.id))
}
