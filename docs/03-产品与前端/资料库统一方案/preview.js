const $ = (s) => document.querySelector(s);
const esc = (s) => String(s).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const icon = name => `<svg class="i" viewBox="0 0 24 24" aria-hidden="true">${window.FOX_ICONS[name] || window.FOX_ICONS.files}</svg>`;
const asset = '../../../apps/desktop/public';
const libraries = [
  {id:'folder',name:'项目资料',kind:'folder',label:'本地文件夹',path:'D:/工作/项目资料',state:'ready'},
  {id:'managed',name:'Fox 使用指南',kind:'managed',label:'已有资料库',path:'Fox 托管副本',state:'ready'},
  {id:'remote',name:'团队资料',kind:'remote',label:'远程 · 只读',path:'示例团队连接',state:'ready'},
  {id:'empty',name:'学习笔记',kind:'folder',label:'本地文件夹',path:'D:/学习/笔记',state:'empty'}
];
const files = [
  {id:1,name:'项目说明.md',folder:'项目文档',type:'MD',size:'8 KB',state:'可问答',text:'项目说明\n这个示例展示文件夹成为资料库后的阅读方式。\n资料保留在电脑原来的位置。Fox 扫描目录后，文件可以立即浏览；完成整理的文件可以用于问答。\n更新文件时，只处理变化的内容。'},
  {id:2,name:'需求讨论.docx',folder:'项目文档',type:'DOCX',size:'126 KB',state:'可问答',text:'需求讨论\n将文件浏览和知识整理放在同一页。\n目录负责定位，列表负责筛选，预览负责阅读。无需切换到另一个入口再导入一次。'},
  {id:3,name:'调研记录.pdf',folder:'参考资料',type:'PDF',size:'2.4 MB',state:'整理失败',text:'这份示例文件带有密码，暂时无法读取内容。'},
  {id:4,name:'预算汇总.xlsx',folder:'参考资料',type:'XLSX',size:'42 KB',state:'待整理',text:'预算汇总\n示例预览：项目、预算、备注。\n原型使用文本代替表格查看器；正式接入复用现有表格预览组件。'}
];
let current = null, tab = 'files', folder = '全部文件', filter = 'all', query = '', selected = new Set(), toastTimer, opener;
const currentLibrary = () => libraries.find(l => l.id === current);
const kindIcon = kind => kind === 'remote' ? 'database' : kind === 'folder' ? 'folder' : 'book-copy';
function toast(text) { $('#toast').textContent = text; $('#toast').classList.add('visible'); clearTimeout(toastTimer); toastTimer = setTimeout(() => $('#toast').classList.remove('visible'), 3600); }
function openPanel(title, body, footer = '') {
  opener = document.activeElement;
  $('#panel').classList.toggle('drawer', title === '资料库设置');
  $('#panel').innerHTML = `<div class="dialog-head"><h2 id="panel-title">${esc(title)}</h2><button data-action="close" aria-label="关闭">${icon('x')}</button></div>${body}${footer ? `<div class="dialog-footer">${footer}</div>` : ''}`;
  if ($('#auto-sync')) $('#auto-sync').checked = currentLibrary().autoSync !== false;
  $('#panel').showModal();
}
function closePanel() { $('#panel').close(); }
$('#panel').addEventListener('close', () => opener?.isConnected && opener.focus());
function openLibrary(id) { current = id; query = ''; tab = 'files'; folder = '全部文件'; selected.clear(); render(); }
function nav() { $('#home-nav').innerHTML = `${icon('book-copy')}资料库`; $('#home-nav').classList.toggle('active', !current); $('#library-nav').innerHTML = libraries.map(l => `<button class="nav-item ${l.id === current ? 'active' : ''}" data-library="${l.id}">${icon(kindIcon(l.kind))}${esc(l.name)}</button>`).join(''); }
function render() { nav(); if (current) renderDetail(); else renderHome(); }
function renderHome() {
  $('#workspace').innerHTML = `<div class="page-head"><div><h2>资料库</h2><p>本地与远程资料，都在这里。</p></div><button class="primary" data-action="add">${icon('plus')}添加资料库</button></div><div class="toolbar"><input id="library-search" aria-label="搜索资料库" placeholder="搜索资料库名称" value="${esc(query)}">${[['all','全部'],['local','本地'],['remote','远程']].map(([id,label]) => `<button class="filter ${filter === id ? 'active' : ''}" data-filter="${id}">${label}</button>`).join('')}</div><div id="library-results"></div><div class="welcome"><img src="${asset}/mascot/fox_sit.png" alt="小狐"><p>添加一个文件夹，就是一个资料库。<br>原文件保持原位，整理进度随时可见。</p></div>`;
  libraryResults();
  $('#library-search').addEventListener('input', e => { query = e.target.value; libraryResults(); });
}
function libraryResults() {
  const visible = libraries.filter(l => l.name.toLowerCase().includes(query.toLowerCase()) && (filter === 'all' || (filter === 'remote' ? l.kind === 'remote' : l.kind !== 'remote')));
  $('#library-results').innerHTML = visible.length ? `<div class="library-grid">${visible.map(l => `<button class="library" data-library="${l.id}"><div class="library-top">${icon(kindIcon(l.kind))}<small>${esc(l.label)}</small></div><h3>${esc(l.name)}</h3><p>${esc(l.path)}</p><div class="library-footer"><span>${l.state === 'empty' ? '还没有文件' : l.kind === 'remote' ? '由远程服务管理' : '4 份示例文件'}</span><span>${l.state === 'empty' ? '等待添加' : l.kind === 'remote' ? '已连接' : '可浏览'}</span></div></button>`).join('')}</div>` : `<div class="empty"><h3>没有找到资料库</h3><p>换个名称，或切换来源范围。</p><button data-action="clear-search">清空筛选</button></div>`;
}
function detailFiles() { const l = currentLibrary(); return l.state === 'empty' ? [] : files.filter(f => (folder === '全部文件' || f.folder === folder) && f.name.toLowerCase().includes(query.toLowerCase())); }
function renderDetail() {
  const l = currentLibrary();
  $('#workspace').innerHTML = `<div class="page-head"><div><button class="filter" data-action="home">${icon('chevron-left')}全部资料库</button><h2 style="margin-top:16px">${esc(l.name)}</h2><p>${esc(l.label)} · ${esc(l.path)}</p></div><div class="actions"><button data-action="settings">${icon('settings')}设置</button><button class="primary" data-action="ask" ${l.state === 'expired' || l.state === 'empty' ? 'disabled' : ''}>问问资料库</button></div></div><div class="tabs"><button class="${tab === 'files' ? 'active' : ''}" data-tab="files">文件</button><button class="${tab === 'jobs' ? 'active' : ''}" data-tab="jobs">${l.kind === 'remote' ? '连接记录' : '整理记录'}</button></div><div class="toolbar"><label>演示状态 <select id="scenario"><option value="ready">正常浏览</option><option value="empty">空资料库</option>${l.kind === 'remote' ? '<option value="expired">连接失效</option>' : '<option value="processing">正在整理</option><option value="offline">文件夹失联</option>'}</select></label></div><div id="detail-body"></div>`;
  $('#scenario').value = l.state; $('#scenario').addEventListener('change', e => { l.state = e.target.value; selected.clear(); renderDetail(); });
  if (l.state === 'expired' || l.state === 'offline') { $('#detail-body').innerHTML = `<div class="empty"><img src="${asset}/mascot/fox_search.png" alt=""><h3>${l.state === 'expired' ? '远程连接已失效' : '找不到原文件夹'}</h3><p>${l.state === 'expired' ? '请重新连接后再查看和检索资料。' : '文件可能已移动。重新定位后继续使用，历史引用会保留。'}</p><button class="primary" data-action="reconnect">${l.state === 'expired' ? '重新连接（演示）' : '重新定位（演示）'}</button></div>`; return; }
  if (tab === 'jobs') { renderJobs(); return; }
  $('#detail-body').innerHTML = `${l.kind === 'remote' ? '<div class="notice">此连接只有读取权限，文件由远程服务管理。当前示例支持浏览、预览和检索。</div>' : l.state === 'processing' ? '<div class="notice">正在整理 2 / 4 份文件。可以先浏览；问答仅使用已完成的文件。<button data-tab="jobs">查看进度</button></div>' : ''}<div class="browser"><nav class="folders" aria-label="目录">${['全部文件','项目文档','参考资料'].map(n => `<button class="${folder === n ? 'active' : ''}" data-folder="${n}">${icon('folder')}${n}</button>`).join('')}</nav><div class="file-area"><div class="toolbar" style="margin-top:0"><input id="file-search" aria-label="搜索文件名" placeholder="搜索此资料库的文件名" value="${esc(query)}"><select id="sort" aria-label="文件排序"><option value="name">按名称</option><option value="type">按类型</option></select></div><div id="file-results"></div></div></div>`;
  fileResults(); $('#file-search').addEventListener('input', e => { query = e.target.value; fileResults(); }); $('#sort').addEventListener('change', fileResults);
}
function fileResults() {
  const l = currentLibrary(), visible = detailFiles().sort((a,b) => ($('#sort').value === 'type' ? a.type.localeCompare(b.type) : a.name.localeCompare(b.name,'zh-CN')));
  $('#file-results').innerHTML = visible.length ? `<div class="batch"><span>已选 ${selected.size} 项</span>${selected.size ? `<button data-action="batch">${l.kind === 'remote' ? '查看引用范围' : '整理所选（演示）'}</button><button data-action="deselect">取消选择</button>` : '<small>选择文件可批量操作</small>'}</div><div class="file-row row-head"><span></span><span>文件名称</span><span>状态</span><small>大小</small></div>${visible.map(f => `<div class="file-row"><input type="checkbox" aria-label="选择 ${esc(f.name)}" data-select="${f.id}" ${selected.has(f.id) ? 'checked' : ''}><button data-file="${f.id}">${icon('file-text')}<span>${esc(f.name)}</span></button><small>${l.kind === 'remote' ? '远程文件' : f.state}</small><small>${f.size}</small></div>`).join('')}` : `<div class="empty"><img src="${asset}/mascot/fox_search.png" alt=""><h3>${query ? '没有匹配的文件' : '这里还没有文件'}</h3><p>${query ? '试试其他文件名。' : l.kind === 'folder' ? '将文件放入原文件夹后，再刷新目录。' : '资料添加后会显示在这里。'}</p><button data-action="${query ? 'clear-files' : 'refresh'}">${query ? '清空搜索' : '刷新目录（演示）'}</button></div>`;
}
function renderJobs() { const l = currentLibrary(); $('#detail-body').innerHTML = l.kind === 'remote' ? '<div class="job"><div><b>已连接团队资料</b><p>读取权限 · 示例记录。远程索引由服务管理。</p></div></div>' : `<div class="job"><div><b>${l.state === 'processing' ? '正在整理资料' : '最近一次整理'}</b><p>示例记录 · 2 份可问答，1 份待整理，1 份失败</p>${l.state === 'processing' ? '<div class="progress"><span></span></div>' : ''}</div>${l.state === 'processing' ? '<button data-action="finish">完成本次演示</button>' : ''}</div><div class="job"><div><b>调研记录.pdf</b><p>${files[2].state === '整理失败' ? '文件带有密码，无法读取。请提供可读取的版本后重试。' : '已重新读取，当前可以用于问答。'}</p></div><button data-action="retry">重试（演示）</button></div>`; }
function addPanel() { openPanel('添加资料库', `<p>选择资料的来源。文件夹保留原位，远程资料继续由原服务管理。</p><label class="form-row">来源<select id="add-kind"><option value="folder">本地文件夹</option><option value="remote">远程连接</option></select></label><label class="form-row">资料库名称<input id="add-name" value="产品手册" maxlength="60"></label><label class="form-row"><span id="path-label">文件夹路径（示例）</span><input id="add-path" value="D:/工作/产品手册"></label><label class="check-row" id="index-option"><input id="add-index" type="checkbox" checked>整理后用于问答</label><p id="add-hint">演示使用本地模型，无需上传资料。正式接入时，根据所选模型显示处理位置。</p><p id="add-error" role="alert"></p>`, '<button data-action="close">取消</button><button class="primary" data-action="create">添加（演示）</button>'); $('#add-kind').addEventListener('change', e => { const remote = e.target.value === 'remote'; $('#path-label').textContent = remote ? '连接名称（示例）' : '文件夹路径（示例）'; $('#add-path').value = remote ? '团队知识服务' : 'D:/工作/产品手册'; $('#index-option').hidden = remote; $('#add-hint').textContent = remote ? '正式接入将选择已授权的连接，并检查浏览与检索权限。此处不收集密钥。' : '演示使用本地模型，无需上传资料。正式接入时，根据所选模型显示处理位置。'; }); }
function settings() { const l = currentLibrary(); openPanel('资料库设置', `<label class="form-row">名称<input id="rename" value="${esc(l.name)}" maxlength="60"></label><p>来源：${esc(l.label)}<br>${esc(l.path)}</p>${l.kind === 'folder' ? '<label class="check-row"><input id="auto-sync" type="checkbox" checked>自动整理新增与修改的文件</label><p>处理位置：本机（示例）。排除隐藏目录和临时文件；规则会在正式设置中开放。</p>' : l.kind === 'remote' ? '<p>当前权限：浏览、预览、检索。上传和删除不可用。</p>' : '<p>保留现有文档、分段和索引。旧资料库不自动改为文件夹镜像。</p>'}<p>“从 Fox 移除”保留原文件和远程资料。托管副本删除需要单独确认。</p>`, `${l.kind !== 'managed' ? '<button data-action="remove-confirm">从 Fox 移除</button>' : ''}<button class="primary" data-action="save-settings">保存（演示）</button>`); }
function preview(id) { const f = files.find(f => f.id === id), l = currentLibrary(); openPanel(f.name, `<div class="reader-meta">${esc(l.name)} / ${esc(f.folder)} · ${f.size} · 示例内容</div><article class="reader">${f.id === 3 && f.state === '整理失败' ? '<h3>暂时无法预览</h3><p>此文件带有密码。其他文件仍可浏览。</p>' : f.text.split('\n').map((line,i) => i ? `<p>${esc(line)}</p>` : `<h3>${esc(line)}</h3>`).join('')}</article>`, `${f.id === 3 && f.state === '整理失败' ? '<button data-action="retry-file">重试（演示）</button>' : '<button data-action="ask-file">问这份资料</button>'}<button data-action="close">返回列表</button>`); }
document.addEventListener('change', e => { if (e.target.dataset.select) { const id = Number(e.target.dataset.select); e.target.checked ? selected.add(id) : selected.delete(id); fileResults(); document.querySelector(`[data-select="${id}"]`)?.focus(); } });
document.addEventListener('click', e => {
  const b = e.target.closest('button'); if (!b) return;
  if (b.dataset.library) return openLibrary(b.dataset.library);
  if (b.dataset.filter) { filter = b.dataset.filter; return renderHome(); }
  if (b.dataset.tab) { tab = b.dataset.tab; return renderDetail(); }
  if (b.dataset.folder) { folder = b.dataset.folder; selected.clear(); return renderDetail(); }
  if (b.dataset.file) return preview(Number(b.dataset.file));
  const l = currentLibrary();
  switch(b.dataset.action) {
    case 'home': current = null; query = ''; selected.clear(); render(); break;
    case 'add': addPanel(); break;
    case 'close': closePanel(); break;
    case 'clear-search': query = ''; filter = 'all'; renderHome(); break;
    case 'clear-files': query = ''; renderDetail(); break;
    case 'deselect': selected.clear(); fileResults(); break;
    case 'settings': settings(); break;
    case 'save-settings': if (!$('#rename').value.trim()) return toast('请填写资料库名称'); l.name = $('#rename').value.trim(); l.autoSync = $('#auto-sync')?.checked; closePanel(); render(); toast('演示设置已保存'); break;
    case 'refresh': toast('目录已刷新，示例资料库仍为空'); break;
    case 'reconnect': l.state = 'ready'; renderDetail(); toast('示例连接已恢复'); break;
    case 'finish': l.state = 'ready'; renderDetail(); toast('演示整理已结束，失败文件仍可单独处理'); break;
    case 'retry': case 'retry-file': files[2].state = '可问答'; files[2].text = '调研记录\n示例：已替换为可读取版本，重新整理完成。'; if ($('#panel').open) closePanel(); renderDetail(); toast('演示重试完成'); break;
    case 'batch': if (l.kind === 'remote') toast(`示例引用范围：${selected.size} 份远程文件，原文件不变`); else { l.state = 'processing'; selected.clear(); tab = 'jobs'; renderDetail(); } break;
    case 'ask': case 'ask-file': if ($('#panel').open) closePanel(); openPanel('问答入口预览', '<p>正式接入后，在对话中附带当前资料库或文件的引用。</p><p>本原型不调用模型，不生成回答。未整理文件会排除在问答范围之外，并显示原因。</p>', '<button data-action="close">返回资料</button>'); break;
    case 'remove-confirm': closePanel(); openPanel('从 Fox 移除资料库', `<p>将移除「${esc(l.name)}」的本地关联。${l.kind === 'folder' ? '原文件夹和文件保持不变；正式接入同时清理该库索引。' : '远程资料不会删除。'}</p>`, '<button data-action="close">取消</button><button data-action="remove">确认移除（演示）</button>'); break;
    case 'remove': libraries.splice(libraries.indexOf(l),1); current = null; closePanel(); render(); toast('已移除示例关联，原文件不变'); break;
    case 'create': {
      const name = $('#add-name').value.trim(), path = $('#add-path').value.trim(), kind = $('#add-kind').value;
      if (!name || !path) { $('#add-error').textContent = '请填写名称和来源。'; return; }
      const normalized = path.replaceAll('\\','/').replace(/\/+$/,'').toLowerCase();
      const duplicate = libraries.find(x => x.kind === kind && x.path.toLowerCase().replace(/\/+$/,'') === normalized);
      if (duplicate) { closePanel(); openLibrary(duplicate.id); toast('这个来源已添加，已打开原资料库'); return; }
      if (kind === 'folder' && libraries.some(x => x.kind === kind && (normalized.startsWith(x.path.toLowerCase() + '/') || x.path.toLowerCase().startsWith(normalized + '/')))) { $('#add-error').textContent = '此路径与已有资料库重叠。请改选目录，或返回已有资料库。'; return; }
      const item = {id:'demo-'+Date.now(),name,path:kind === 'folder' ? path.replaceAll('\\','/') : path,kind,label:kind === 'folder' ? '本地文件夹' : '远程 · 只读',state:kind === 'folder' ? ($('#add-index').checked ? 'processing' : 'empty') : 'ready'};
      libraries.push(item); closePanel(); openLibrary(item.id); toast('已添加示例资料库'); break;
    }
  }
});
$('#theme').addEventListener('change', e => document.documentElement.dataset.colorTheme = e.target.value);
$('#mode').addEventListener('click', () => { const dark = document.documentElement.classList.toggle('dark'); $('#mode').textContent = dark ? '切换浅色' : '切换深色'; });
render();
