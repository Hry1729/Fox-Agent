import { describe, expect, test } from 'bun:test'
import {
  captureManagementReturnRoutes,
  localKnowledgeBackRoute,
  localKnowledgeViewForWorkspace,
  managementExitRoute,
  managementSection,
  SETTINGS_WORKSPACE_VIEWS,
  type ManagementReturnRoutes,
  type WorkspaceReturnRoute,
  workspaceViewForLocalKnowledge,
} from '../src/features/workspace/management-navigation'

const chatRoute: WorkspaceReturnRoute = { view: 'chat', entityId: null, documentId: null }

function routes(overrides: Partial<ManagementReturnRoutes> = {}): ManagementReturnRoutes {
  return {
    settings: chatRoute,
    knowledge: chatRoute,
    ...overrides,
  }
}

describe('workspace navigation', () => {
  test('maps local knowledge pages to workspace views', () => {
    expect(localKnowledgeViewForWorkspace('local-knowledge-home')).toBe('home')
    expect(localKnowledgeViewForWorkspace('local-files')).toBe('files')
    expect(localKnowledgeViewForWorkspace('local-knowledge')).toBe('list')
    expect(localKnowledgeViewForWorkspace('local-knowledge-detail')).toBe('detail')
    expect(localKnowledgeViewForWorkspace('local-knowledge-documents')).toBe('documents')
    expect(localKnowledgeViewForWorkspace('local-knowledge-import')).toBe('import')
    expect(localKnowledgeViewForWorkspace('local-knowledge-jobs')).toBe('jobs')
    expect(localKnowledgeViewForWorkspace('knowledge')).toBeNull()
    expect(workspaceViewForLocalKnowledge('detail')).toBe('local-knowledge-detail')
    expect(workspaceViewForLocalKnowledge('files')).toBe('local-files')
    expect(workspaceViewForLocalKnowledge('home')).toBe('local-knowledge-home')
  })

  test('returns from local knowledge detail pages to the correct parent', () => {
    expect(localKnowledgeBackRoute('local-knowledge-detail', 'kb-1')).toEqual({ view: 'local-knowledge' })
    expect(localKnowledgeBackRoute('local-knowledge-documents', 'kb-1')).toEqual({ view: 'local-knowledge-detail', entityId: 'kb-1' })
    expect(localKnowledgeBackRoute('local-knowledge-home', null)).toEqual({ view: 'chat' })
    expect(localKnowledgeBackRoute('local-knowledge', null)).toEqual({ view: 'local-knowledge-home' })
    expect(localKnowledgeBackRoute('local-files', null)).toEqual({ view: 'local-knowledge' })
  })

  test('keeps the original return route across shortcut settings navigation', () => {
    const documentRoute: WorkspaceReturnRoute = { view: 'local-knowledge-documents', entityId: 'kb-1', documentId: 'doc-1' }
    expect(SETTINGS_WORKSPACE_VIEWS).toContain('settings-shortcuts')
    expect(managementSection('settings-shortcuts')).toBe('settings')
    const entered = captureManagementReturnRoutes(routes(), documentRoute, 'settings-shortcuts')
    expect(managementExitRoute('settings-shortcuts', entered)).toEqual(documentRoute)
    const switched = captureManagementReturnRoutes(entered, { view: 'settings-shortcuts', entityId: null, documentId: null }, 'settings-ai')
    expect(managementExitRoute('settings-ai', switched)).toEqual(documentRoute)
  })

  test('keeps plugins standalone while local knowledge uses management return handling', () => {
    expect(SETTINGS_WORKSPACE_VIEWS).not.toContain('plugins')
    expect(managementSection('plugins')).toBeNull()
    expect(managementSection('local-knowledge-detail')).toBe('knowledge')

    const localKnowledgeRoute: WorkspaceReturnRoute = { view: 'local-knowledge-detail', entityId: 'kb-1', documentId: null }
    const next = captureManagementReturnRoutes(routes(), localKnowledgeRoute, 'settings')
    expect(next.settings).toEqual(localKnowledgeRoute)
    expect(managementExitRoute('settings-ai', next)).toEqual(localKnowledgeRoute)
  })
})
