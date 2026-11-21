// Loopback OpenAI-compatible responses for the native coordination acceptance test.
import { createServer } from 'node:http'
import { appendFileSync } from 'node:fs'

const [log, recipientId] = process.argv.slice(2)
let sequence = 0

function content(value) {
  return typeof value === 'string' ? value
    : Array.isArray(value) ? value.map(part => part.text ?? '').join('\n') : ''
}

function requireFact(fact, message) {
  if (!fact) throw new Error(message)
}

function answer(body) {
  const messages = body.messages ?? []
  const system = messages.filter(message => message.role === 'system' || message.role === 'developer')
    .map(message => content(message.content)).join('\n')
  const userIndex = messages.findLastIndex(message => message.role === 'user')
  const user = content(messages[userIndex]?.content)
  const toolNames = new Set((body.tools ?? []).map(tool => tool.function?.name ?? tool.name))
  const result = id => messages.slice(userIndex + 1).find(message => message.role === 'tool' && message.tool_call_id === id)
  const call = (id, tool, args) => ({ id, tool, args })
  const note = text => ({ text })
  if (user.includes('FOX_COORD_USER_ANSWER')) {
    requireFact(messages.slice(0, userIndex).some(message => message.tool_calls?.some(tool => tool.function?.name === 'ask_user')),
      'the selected option did not resume the original partner session')
    return note('已按你的选择交付 FOX_COORD_CONSULT_RESULT')
  }
  if (user.includes('伙伴回传') && user.includes('FOX_COORD_CONSULT_STILL_MISSING')) {
    requireFact(toolNames.has('ask_user'), 'the initiating contact could not offer a choice')
    return call('coord-source-question', 'ask_user', {
      explanation: '我请核对员补发明细，现有资料仍然只有汇总。',
      question: '接下来怎么交付？',
      options: ['FOX_COORD_USER_ANSWER 先交汇总', '等到有明细再交'],
    })
  }
  if (user.includes('伙伴回传') && user.includes('FOX_COORD_CONSULT_MISSING')) {
    if (result('coord-source-follow-up')) return note('已向核对员补问。')
    return call('coord-source-follow-up', 'companion_coordinate', {
      targets: [recipientId], instructions: 'FOX_COORD_CONSULT_RESEND 请补发已有明细即可。', consult: true, reply: 'resume',
    })
  }
  if (user.includes('伙伴回传') && user.includes('FOX_COORD_WORK_RESULT')) {
    return note('已汇总 FOX_COORD_WORK_RESULT')
  }
  if (user.includes('FOX_COORD_WORK_BRIEF')) {
    requireFact(!JSON.stringify(body).includes('FOX_ORIGIN_PRIVATE_CANARY'), 'source private text reached recipient HTTP request')
    const read = result('coord-receiver-read')
    if (!read) return call('coord-receiver-read', 'read', { path: 'proof.txt' })
    requireFact(content(read.content).includes('FOX_TARGET_FILE_PROOF'), 'recipient did not read its own file')
    if (!result('coord-receiver-write')) {
      return call('coord-receiver-write', 'write_file', { path: 'confirmed.txt', content: 'FOX_COORD_APPROVED_WRITE' })
    }
    return note('FOX_COORD_WORK_RESULT FOX_TARGET_FILE_PROOF')
  }
  if (/FOX_COORD_CONSULT_(?:BRIEF|RESEND)/.test(user)) {
    requireFact(!toolNames.has('write_file'), 'consult advertised a write tool')
    requireFact(!toolNames.has('companion_coordinate'), 'consult advertised a delegation tool')
    requireFact(!toolNames.has('ask_user'), 'the receiving contact can ask the user directly')
    requireFact(!JSON.stringify(body).includes('FOX_ORIGIN_PRIVATE_CANARY'), 'source private text reached consultation HTTP request')
    if (user.includes('FOX_COORD_CONSULT_RESEND')) return note('FOX_COORD_CONSULT_STILL_MISSING 我重新查了，现有资料只有汇总。')
    return note('NEEDS INPUT — FOX_COORD_CONSULT_MISSING 缺少明细。')
  }
  if (user.includes('FOX_COORD_CHIEF_CREATE') || user.includes('FOX_COORD_CHIEF_RECREATE')) {
    if (user.includes('FOX_COORD_CHIEF_RECREATE')) {
      requireFact(user.includes('"status":"undone"'), 'reused chief session did not receive persisted undo state')
    }
    requireFact(toolNames.has('companion_create'), 'default companion did not advertise management tool')
    const id = user.includes('FOX_COORD_CHIEF_RECREATE') ? 'coord-chief-recreate' : 'coord-chief-create'
    if (result(id)) return note('FOX_COORD_CHIEF_CREATED')
    return call(id, 'companion_create', { name: '验收新伙伴', duty: '整理验收报告' })
  }
  if (user.includes('FOX_COORD_CHIEF_ARCHIVE')) {
    if (user.includes('FOX_COORD_CHIEF_ARCHIVE_AGAIN')) {
      const state = user.split('\n').find(line => line.startsWith('{"totalCount":'))
      requireFact(state && JSON.parse(state).actions.some(action => action.kind === 'archive' && action.status === 'dismissed'),
        'reused chief session did not receive persisted archive cancellation')
    }
    const id = user.includes('FOX_COORD_CHIEF_ARCHIVE_AGAIN') ? 'coord-chief-archive-again' : 'coord-chief-archive'
    if (result(id)) return note('FOX_COORD_CHIEF_ARCHIVE_PENDING')
    return call(id, 'companion_propose_archive', { target: '验收新伙伴' })
  }
  if (user.includes('FOX_COORD_ORDINARY_SCOPE')) {
    requireFact(!toolNames.has('companion_coordinate'), 'ordinary work chat advertised delegation to companions')
    for (const name of ['child_agent_list', 'child_run_start', 'child_run_collect', 'child_run_cancel']) {
      requireFact(toolNames.has(name), `ordinary work chat omitted ${name}`)
    }
    const startTool = (body.tools ?? []).find(tool => (tool.function?.name ?? tool.name) === 'child_run_start')
    requireFact(JSON.stringify(startTool).includes('expert_consultation'), 'ordinary work chat omitted expert consultation mode')
    return note('FOX_COORD_ORDINARY_SCOPE_OK')
  }
  const consult = user.includes('FOX_COORD_ROOT_CONSULT')
  if (consult || user.includes('FOX_COORD_ROOT_WORK')) {
    requireFact(toolNames.has('companion_coordinate'), 'companion chat did not advertise coordination')
    if (!consult) requireFact(!toolNames.has('companion_create'), 'regular companion advertised chief management')
    if (result('coord-source-submit')) return note('派活已排队')
    if (!result('coord-source-list')) return call('coord-source-list', 'companion_list', {})
    requireFact(content(result('coord-source-list').content).includes(recipientId), 'real roster omitted the recipient')
    return call('coord-source-submit', 'companion_coordinate', {
      targets: [recipientId],
      instructions: consult ? 'FOX_COORD_CONSULT_BRIEF' : 'FOX_COORD_WORK_BRIEF',
      consult, title: consult ? '验收咨询' : '验收协作',
    })
  }
  throw new Error('unrecognized coordination model turn')
}

const server = createServer(async (request, response) => {
  let raw = ''
  for await (const piece of request) raw += piece
  try {
    const body = JSON.parse(raw)
    appendFileSync(log, JSON.stringify({ body }) + '\n')
    const next = answer(body)
    const id = 'coordination-fixture-' + ++sequence
    const delta = next.tool
      ? { role: 'assistant', tool_calls: [{ index: 0, id: next.id, type: 'function', function: { name: next.tool, arguments: JSON.stringify(next.args) } }] }
      : { role: 'assistant', content: next.text }
    const chunk = choice => ({ id, object: 'chat.completion.chunk', model: 'fox-coordination-fixture', created: 1, choices: [choice] })
    response.writeHead(200, { 'content-type': 'text/event-stream' })
    response.write('data: ' + JSON.stringify(chunk({ index: 0, delta, finish_reason: null })) + '\n\n')
    response.write('data: ' + JSON.stringify(chunk({ index: 0, delta: {}, finish_reason: next.tool ? 'tool_calls' : 'stop' })) + '\n\n')
    response.write('data: ' + JSON.stringify({
      ...chunk(), choices: [], usage: { prompt_tokens: 48, completion_tokens: 12, total_tokens: 60 },
    }) + '\n\n')
    response.end('data: [DONE]\n\n')
  } catch (error) {
    appendFileSync(log, JSON.stringify({ error: error.message }) + '\n')
    response.writeHead(500, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ error: { message: error.message } }))
  }
})
server.listen(0, '127.0.0.1', () => console.log('http://127.0.0.1:' + server.address().port + '/v1'))
process.stdin.resume()
process.stdin.on('end', () => { server.closeAllConnections(); server.close() })
