import { useState } from 'react'
import { ArrowLeft, ArrowUpRight, Check, ChevronRight, CircleAlert, FileText, Plus, RotateCcw, Server } from 'lucide-react'
import { Button } from '../src/components/ui/button'
import './service-proposal.css'

type AgentId = 'codex' | 'pi' | 'deepseek'
type AgentState = 'native' | 'unconfigured' | 'absent' | 'error' | 'bound'
type Service = { id: string; name: string; url: string; model: string; protocol: string; compatible: AgentId[] }
type Form = Service & { preset: string }
const initialServices: Service[] = [
  { id: 'openai', name: 'OpenAI', url: 'https://api.openai.com/v1', model: '示例模型 A', protocol: 'openai-responses', compatible: ['codex', 'pi'] },
  { id: 'team', name: '团队服务', url: 'https://api.example.com/v1', model: '示例模型 B', protocol: 'openai-completions', compatible: ['pi'] },
]
const agents: { id: AgentId; name: string; detail: string }[] = [
  { id: 'codex', name: 'Codex', detail: '本机已有配置' },
  { id: 'pi', name: 'Pi', detail: '已安装，尚未配置' },
  { id: 'deepseek', name: 'DeepSeek', detail: '未安装' },
]
const statusText: Record<AgentState, string> = { native: '本机已有配置', unconfigured: '已安装 · 未配置', absent: '未安装', error: '检测失败', bound: '演示已配置 · 未应用' }
const backendTasks = [
  ['安装与状态检测', '现有 installed 只检查 Fox 受管程序或显式路径。补充系统 CLI 发现，区分“检测到安装”与“Fox 可以启动”；读取失败单独返回。'],
  ['只读检测与应用分开', '现有 model_connection 可能发起网络验证并生成私有配置。先返回状态和配置摘要，用户点击验证或应用后再执行。'],
  ['按 Agent 选择来源与服务', '当前外部 Agent 主要跟随全局默认供应商。增加每个 Agent 的来源、服务、入口和模型绑定；Pi 已有的来源选择可复用。'],
  ['统一导入范围', '已具备 Codex、Pi、dsh 的白名单导入和覆盖备份。补齐统一预览、范围选择、冲突说明及新项目复用；现有界面原生导入只暴露给 Pi。'],
  ['兼容与验证结果', '复用现有协议匹配、连接验证和系统凭据库。区分配置保存、模型请求通过、Agent 输出及工具调用通过。系统 Agent 的写配置另设范围。'],
]

export function ServiceProposal() {
  const [services, setServices] = useState(initialServices)
  const [states, setStates] = useState<Record<AgentId, AgentState>>({ codex: 'native', pi: 'unconfigured', deepseek: 'absent' })
  const [binding, setBinding] = useState<Partial<Record<AgentId, string>>>({})
  const [activeAgent, setActiveAgent] = useState<AgentId | null>(null)
  const [source, setSource] = useState<'native' | 'service'>('service')
  const [selectedService, setSelectedService] = useState('openai')
  const [scope, setScope] = useState('current')
  const [form, setForm] = useState<Form | null>(null)
  const [formTest, setFormTest] = useState(false)
  const [message, setMessage] = useState('')
  const [scenario, setScenario] = useState('initial')

  const openAgent = (id: AgentId) => {
    setActiveAgent(activeAgent === id ? null : id)
    const boundService = services.find(service => service.id === binding[id] && service.compatible.includes(id))
    setSource(states[id] === 'native' || binding[id] === 'native' ? 'native' : 'service')
    setSelectedService(boundService?.id ?? services.find(service => service.compatible.includes(id))?.id ?? '')
    setScope('current')
    setMessage('')
  }
  const editService = (service?: Service) => {
    setForm(service ? { ...service, preset: service.id === 'openai' ? 'openai' : 'custom' } : { id: '', name: 'OpenAI', url: 'https://api.openai.com/v1', model: '示例模型 A', protocol: 'openai-responses', compatible: ['codex', 'pi'], preset: 'openai' })
    setFormTest(false)
    setMessage('')
  }
  const changeForm = (patch: Partial<Form>) => {
    if (form) setForm({ ...form, ...patch })
    setFormTest(false)
  }
  const applyPreset = (preset: string) => {
    if (!form) return
    if (preset === 'openai') setForm({ ...form, preset, name: 'OpenAI', url: 'https://api.openai.com/v1', model: '示例模型 A', protocol: 'openai-responses', compatible: ['codex', 'pi'] })
    else if (preset === 'deepseek') setForm({ ...form, preset, name: 'DeepSeek 官方', url: 'https://api.deepseek.com/anthropic', model: '示例模型 C', protocol: 'anthropic-messages', compatible: ['pi', 'deepseek'] })
    else setForm({ ...form, preset, name: '', url: '', model: '', protocol: 'openai-completions', compatible: ['pi'] })
    setFormTest(false)
  }
  const saveService = () => {
    if (!form) return
    const service: Service = { ...form, id: form.id || `demo-${services.length + 1}`, compatible: form.protocol === 'openai-responses' ? ['codex', 'pi'] : form.protocol === 'anthropic-messages' ? ['pi', 'deepseek'] : ['pi'] }
    setServices(services.some(item => item.id === service.id) ? services.map(item => item.id === service.id ? service : item) : [...services, service])
    setForm(null)
    setMessage(`“${service.name}”已记录在本次演示中；没有保存到 Fox，也没有验证实际连接。`)
  }
  const applyAgent = () => {
    if (!activeAgent) return
    const service = services.find(item => item.id === selectedService)
    if (source === 'service' && !service?.compatible.includes(activeAgent)) return
    setStates({ ...states, [activeAgent]: 'bound' })
    setBinding({ ...binding, [activeAgent]: source === 'native' ? 'native' : selectedService })
    setMessage(source === 'native' ? '演示：已展示导入后的状态。实际导入需要后端；本机文件和 Fox 配置均未修改。' : '演示：已展示应用后的状态。实际绑定与验证需要后端；Fox 配置均未修改。')
    setActiveAgent(null)
  }
  const selected = services.find(item => item.id === selectedService)
  const compatible = activeAgent !== null && selected?.compatible.includes(activeAgent)

  return <article className="service-proposal">
    <div className="sp-demo"><CircleAlert aria-hidden="true" /><span><strong>第 9 项 · 交互方案演示</strong>所有安装状态、配置与验证结果均为模拟；不读取本机，不接收真实密钥，不修改配置。</span></div>
    <header className="sp-page-head"><div><h2>模型</h2><p>先连接模型服务，再为每个 Agent 选择使用方式。</p></div>{!form && <Button onClick={() => editService()} size="sm"><Plus aria-hidden="true" />添加服务</Button>}</header>
    {message && <div className="sp-notice" role="status"><Check aria-hidden="true" /><span>{message}</span></div>}
    {form ? <section className="sp-editor" aria-labelledby="sp-editor-title">
      <button className="sp-back" type="button" onClick={() => setForm(null)}><ArrowLeft aria-hidden="true" />返回模型服务</button>
      <div className="sp-editor-head"><h3 id="sp-editor-title">{form.id ? '编辑服务' : '添加服务'}</h3><span>与编辑使用同一套页内表单</span></div>
      <div className="sp-form-grid">
        <label><span>服务商</span><select value={form.preset} onChange={event => applyPreset(event.target.value)}><option value="openai">OpenAI</option><option value="deepseek">DeepSeek 官方</option><option value="custom">自定义服务</option></select></label>
        <label><span>服务名称</span><input value={form.name} placeholder="例如，团队服务" onChange={event => changeForm({ name: event.target.value })} /></label>
        <label className="sp-wide"><span>服务地址</span><input type="url" value={form.url} placeholder="https://api.example.com/v1" onChange={event => changeForm({ url: event.target.value })} /></label>
        <label className="sp-wide"><span>API Key</span><input value="演示模式不接收真实密钥" disabled /><small>正式版复用系统凭据库；已保存的密钥留空保留。</small></label>
        <label className="sp-wide"><span>默认模型</span><input value={form.model} placeholder="填写服务商提供的模型 ID" onChange={event => changeForm({ model: event.target.value })} /><small>模型在当前页面编辑，减少弹窗来回切换。</small></label>
      </div>
      <details className="sp-advanced"><summary><ChevronRight aria-hidden="true" />高级选项<span>连接协议、独立入口与模型参数</span></summary><div><label><span>连接协议</span><select value={form.protocol} onChange={event => changeForm({ protocol: event.target.value })}><option value="openai-responses">Responses API</option><option value="anthropic-messages">Anthropic Messages</option><option value="openai-completions">Chat Completions</option></select></label><p>正式版按服务商模板与 Agent 能力代填；自定义入口保留此选项。协议相符仍需实际验证，不能保证 Agent 可用。</p></div></details>
      <div className="sp-test-row"><Button variant="outline" size="sm" disabled={!form.url.trim() || !form.model.trim()} onClick={() => setFormTest(true)}>演示验证反馈</Button>{formTest && <span role="status">模拟：模型请求通过；Agent 工具调用尚未验证。</span>}</div>
      <footer className="sp-editor-foot"><Button variant="outline" size="sm" onClick={() => setForm(null)}>取消</Button><Button size="sm" disabled={!form.name.trim() || !form.url.trim() || !form.model.trim()} onClick={saveService}>保存演示配置</Button></footer>
    </section> : <>
      <section className="sp-section" aria-labelledby="sp-services-title"><div className="sp-section-head"><h3 id="sp-services-title">模型服务</h3><span>服务连接只配置一次</span></div><div className="sp-group">
        {services.map(service => <div className="sp-service-row" key={service.id}><span className="sp-service-icon"><Server aria-hidden="true" /></span><div className="sp-row-main"><strong>{service.name}</strong><span>{service.model} · 示例数据</span><small>可匹配 {service.compatible.map(id => agents.find(agent => agent.id === id)?.name).join('、')} · 尚未实测</small></div><Button variant="ghost" size="sm" onClick={() => editService(service)}>编辑</Button></div>)}
      </div></section>
      <section className="sp-section" aria-labelledby="sp-agent-title"><div className="sp-section-head"><h3 id="sp-agent-title">Agent 使用配置</h3><label className="sp-scenario">演示情景<select aria-label="切换演示情景" value={scenario} onChange={event => { const next = event.target.value; setScenario(next); setStates({ codex: 'native', pi: 'unconfigured', deepseek: next === 'failed' ? 'error' : next === 'installed' ? 'unconfigured' : 'absent' }); setBinding({}); setActiveAgent(null); setMessage('') }}><option value="initial">本机配置 / 未配置 / 未安装</option><option value="failed">检测失败</option><option value="installed">DeepSeek 已安装未配置</option></select></label></div><div className="sp-group">
        {agents.map(agent => { const state = states[agent.id]; const bound = binding[agent.id]; return <div className="sp-agent-item" key={agent.id}>
          <div className="sp-agent-row"><div className="sp-row-main"><strong>{agent.name}</strong><span className={state === 'error' ? 'sp-error' : ''}>{statusText[state]}{state === 'bound' && ` · ${bound === 'native' ? '本机配置副本' : services.find(item => item.id === bound)?.name ?? '模型服务'}`}</span></div>
            {state === 'absent' ? <span className="sp-unavailable">安装后可配置</span> : state === 'error' ? <Button variant="outline" size="sm" onClick={() => { setStates({ ...states, [agent.id]: 'absent' }); setMessage('演示：重试后返回“未安装”。没有执行本机检测。') }}><RotateCcw aria-hidden="true" />重试检测</Button> : <Button variant="ghost" size="sm" aria-expanded={activeAgent === agent.id} onClick={() => openAgent(agent.id)}>{state === 'native' ? '查看并导入' : state === 'bound' ? '更改' : '配置'}<ChevronRight aria-hidden="true" /></Button>}
          </div>
          {activeAgent === agent.id && <div className="sp-agent-editor">
            <p className="sp-small-title">{agent.name} 使用哪种配置？</p><div className="sp-source-options">
              {agent.id === 'codex' && <label><input type="radio" name="sp-source" checked={source === 'native'} onChange={() => setSource('native')} /><span><strong>使用本机已有配置</strong><small>查看导入内容，保留原配置文件</small></span></label>}
              <label><input type="radio" name="sp-source" checked={source === 'service'} onChange={() => setSource('service')} /><span><strong>使用 Fox 模型服务</strong><small>选择现有服务，或在这里添加</small></span></label>
            </div>
            {source === 'native' ? <div className="sp-import-preview"><div><FileText aria-hidden="true" /><span>~/.codex/auth.json<br />~/.codex/config.toml</span></div><p>来源：本机 Codex 配置。目标：Fox 内运行的 Codex 私有配置目录。仅展示文件名，不显示密钥。</p><label><span>应用范围</span><select value={scope} onChange={event => setScope(event.target.value)}><option value="current">当前项目</option><option value="new">以后新建的项目（需后端支持）</option></select></label><p>正式版导入前应展示冲突；已有配置默认保留。此演示不执行导入。</p></div> : <div className="sp-bind-form"><label><span>模型服务</span><select value={selectedService} onChange={event => setSelectedService(event.target.value)}><option value="">选择服务</option>{services.map(service => <option key={service.id} value={service.id} disabled={!service.compatible.includes(agent.id)}>{service.name}{service.compatible.includes(agent.id) ? '' : ' · 缺少适配入口'}</option>)}</select></label>{selected && <div className="sp-binding-summary"><strong>{selected.model}</strong><span>{compatible ? '协议可匹配 · 实际连接与 Agent 运行尚未验证' : `此服务没有适用于 ${agent.name} 的入口`}</span></div>}{!services.some(service => service.compatible.includes(agent.id)) && <p>现有服务缺少适配入口，请添加支持 {agent.name} 的服务。</p>}<Button variant="outline" size="sm" onClick={() => editService()}><Plus aria-hidden="true" />添加模型服务</Button><p>作用范围：Fox 内运行的此 Agent。现有对话的生效时机由后端返回。</p></div>}
            <footer className="sp-agent-editor-foot"><Button variant="ghost" size="sm" onClick={() => setActiveAgent(null)}>取消</Button><Button size="sm" disabled={source === 'service' && !compatible} onClick={applyAgent}>{source === 'native' ? '演示导入结果' : '演示应用结果'}</Button></footer>
          </div>}
        </div> })}
      </div><p className="sp-footnote">此页默认管理 Fox 内的 Agent。若以后支持修改系统 Agent 配置，应另设明确入口，并显示文件差异、备份与恢复。</p></section>
    </>}
    <section className="sp-section sp-research" aria-labelledby="sp-dependencies-title"><div className="sp-section-head"><h3 id="sp-dependencies-title">后端需要先完成</h3><span>前端方案暂不接入</span></div><div className="sp-group">{backendTasks.map(([title, detail], index) => <details key={title}><summary><span>{index + 1}. {title}</span><ChevronRight aria-hidden="true" /></summary><p>{detail}</p></details>)}</div><p className="sp-reference">参考 <a href="https://github.com/farion1231/cc-switch/blob/main/docs/user-manual/en/2-providers/2.1-add.md" target="_blank" rel="noreferrer">CC Switch 添加流程<ArrowUpRight aria-hidden="true" /></a>：先选目标应用与预设，再填写必需字段；添加与编辑保持同一结构。</p><p className="sp-reference">补充参考：<a href="https://github.com/yetone/magpie/blob/main/docs/subsystems/agent-wiring.md" target="_blank" rel="noreferrer">Magpie Agent 配置说明<ArrowUpRight aria-hidden="true" /></a>。借鉴逐 Agent 配置、安装发现，以及导入与应用分离。</p></section>
  </article>
}
