import { describe, expect, test } from 'bun:test'
import {
  captureManagementReturnRoutes,
  managementExitRoute,
  localKnowledgeBackRoute,
  remoteKnowledgeBackRoute,
  localKnowledgeViewForWorkspace,
  workspaceViewForLocalKnowledge,
  type ManagementReturnRoutes,
  type WorkspaceReturnRoute,
} from '../src/features/workspace/management-navigation'

const chatRoute: WorkspaceReturnRoute = { view: 'chat', entityId: null, documentId: null }

function initialRoutes(): ManagementReturnRoutes {
  return { settings: chatRoute, knowledge: chatRoute }
}

describe('management navigation', () => {
  test('remote detail returns to the library list instead of exiting the materials workspace', () => {
    const entered = captureManagementReturnRoutes(initialRoutes(), chatRoute, 'local-knowledge-home')
    const listed = captureManagementReturnRoutes(entered, { view: 'local-knowledge-home', entityId: null, documentId: null }, 'knowledge')
    const detailed = captureManagementReturnRoutes(listed, { view: 'knowledge', entityId: null, documentId: null }, 'knowledge-detail')
    expect(managementExitRoute('knowledge-detail', detailed)).toEqual(chatRoute)
    expect(remoteKnowledgeBackRoute('knowledge-detail', 'remote-a')).toEqual({ view: 'knowledge' })
    expect(remoteKnowledgeBackRoute('knowledge-graph', 'remote-a')).toEqual({ view: 'knowledge-detail', entityId: 'remote-a' })
    expect(remoteKnowledgeBackRoute('knowledge-graph')).toEqual({ view: 'knowledge' })
  })
  test('library settings preserve the library route and management return history', () => {
    expect(localKnowledgeViewForWorkspace('local-knowledge-settings')).toBe('settings')
    expect(workspaceViewForLocalKnowledge('settings')).toBe('local-knowledge-settings')
    expect(localKnowledgeBackRoute('local-knowledge-settings', 'kb-a')).toEqual({ view: 'local-knowledge-detail', entityId: 'kb-a' })
    const settingsRoute: WorkspaceReturnRoute = { view: 'local-knowledge-settings', entityId: 'kb-a', documentId: null }
    const enteredKnowledge = captureManagementReturnRoutes(initialRoutes(), chatRoute, 'local-knowledge')
    const librarySettings = captureManagementReturnRoutes(enteredKnowledge, { view: 'local-knowledge-detail', entityId: 'kb-a', documentId: null }, 'local-knowledge-settings')
    expect(managementExitRoute('local-knowledge-settings', librarySettings)).toEqual(chatRoute)
    const appSettings = captureManagementReturnRoutes(librarySettings, settingsRoute, 'settings')
    expect(managementExitRoute('settings', appSettings)).toEqual(settingsRoute)
  })
  test('exits settings to the main page that opened it', () => {
    const knowledgeRoute: WorkspaceReturnRoute = { view: 'knowledge', entityId: null, documentId: null }
    const routes = captureManagementReturnRoutes(initialRoutes(), knowledgeRoute, 'login')

    expect(managementExitRoute('login', routes)).toEqual(knowledgeRoute)
    expect(managementExitRoute('service', routes)).toEqual(knowledgeRoute)
  })

  test('keeps knowledge return history when settings opens inside knowledge', () => {
    const knowledgeRoute: WorkspaceReturnRoute = { view: 'knowledge', entityId: null, documentId: null }
    const enteredKnowledge = captureManagementReturnRoutes(initialRoutes(), chatRoute, 'knowledge')
    const enteredSettings = captureManagementReturnRoutes(enteredKnowledge, knowledgeRoute, 'login')
    const returnedToKnowledge = captureManagementReturnRoutes(enteredSettings, { ...knowledgeRoute, view: 'login' }, 'knowledge')

    expect(managementExitRoute('login', enteredSettings)).toEqual(knowledgeRoute)
    expect(managementExitRoute('knowledge', returnedToKnowledge)).toEqual(chatRoute)
  })

  test('does not overwrite the settings origin within settings', () => {
    const enteredSettings = captureManagementReturnRoutes(initialRoutes(), chatRoute, 'settings')
    const openedLogin = captureManagementReturnRoutes(enteredSettings, { view: 'settings-yuxi', entityId: null, documentId: null }, 'login')

    expect(managementExitRoute('login', openedLogin)).toEqual(chatRoute)
  })
})
