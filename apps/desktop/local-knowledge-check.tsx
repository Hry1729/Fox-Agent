import React from 'react';
import { createRoot } from 'react-dom/client';
import './src/styles/globals.css';
import './src/styles/workbench.css';
import './src/styles/workspace-pages.css';
import { applyColorTheme } from './src/features/settings/color-theme';
import { createMockLocalKnowledgeGateway } from './src/features/local-knowledge/mock-gateway';
import { LocalKnowledgeWorkspace } from './src/features/local-knowledge/local-knowledge-pages';
import presentationFixtureUrl from './src-tauri/tests/fixtures/office-reading.pptx?url';
const query = new URLSearchParams(location.search);
if(query.get('dark')==='1') document.documentElement.classList.add('dark');
applyColorTheme(query.get('theme') || 'graphite');
document.body.style.margin = '0';
const mock = createMockLocalKnowledgeGateway();
function pdfFixture() {
  const content = 'BT /F1 24 Tf 72 720 Td (Fox PDF layout check) Tj ET';
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
    `<< /Length ${content.length} >>\nstream\n${content}\nendstream`,
  ];
  let source = '%PDF-1.4\n';
  const offsets = [0];
  objects.forEach((object, index) => { offsets.push(source.length); source += `${index + 1} 0 obj\n${object}\nendobj\n`; });
  const xref = source.length;
  source += `xref\n0 6\n0000000000 65535 f \n${offsets.slice(1).map((offset) => `${String(offset).padStart(10, '0')} 00000 n \n`).join('')}trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new TextEncoder().encode(source);
}
const binaryKind = query.get('binary');
const binaryBytes = binaryKind === 'pptx'
  ? fetch(presentationFixtureUrl).then((response) => response.arrayBuffer()).then((buffer) => new Uint8Array(buffer))
  : Promise.resolve(pdfFixture());
// The shared mock gateway has no chunk store, so retrieval is stubbed with
// fixture snippets. This checks hit layout, marking and ordering only.
const gateway: any = {
  ...mock,
  listDocuments: async (baseId: string, options?: any) => {
    const page = await mock.listDocuments(baseId, options);
    if (!binaryKind) return page;
    const bytes = await binaryBytes;
    return { ...page, items: page.items.map((doc) => doc.id === 'local-product-readme' ? {
      ...doc, name: `排版检查.${binaryKind}`, extension: binaryKind, sizeBytes: bytes.length,
      mimeType: binaryKind === 'pptx' ? 'application/vnd.openxmlformats-officedocument.presentationml.presentation' : 'application/pdf',
    } : doc) };
  },
  readDocumentFileRange: async (baseId: string, docId: string, start: number, end: number) => {
    if (binaryKind && baseId === 'local-product-docs' && docId === 'local-product-readme') return Array.from((await binaryBytes).slice(start, end + 1));
    return mock.readDocumentFileRange(baseId, docId, start, end);
  },
  testRetrieval: async (knowledgeBaseId: string, query: string, mode: string, topK?: number) => {
    const page = await mock.listDocuments(knowledgeBaseId);
    const term = query.trim();
    return {
      knowledgeBaseId, query, mode, fusionVersion: null,
      timings: { keywordMs: 1, vectorMs: 0, totalMs: 1 },
      items: page.items.slice(0, topK ?? 10).map((doc: any, index: number) => ({
        documentId: doc.id,
        documentName: doc.name,
        relativePath: doc.relativePath,
        chunkId: doc.id + '-c0',
        anchor: '第 ' + (index + 1) + ' 段',
        content: '本段用于检查较长的资料内容如何显示，前面的说明不包含检索词。'.repeat(5) + '现在介绍' + term + '的具体操作方法，可以查看资料并按文件夹整理。'.repeat(5) + '后面再次提到' + term + '，用于确认保留的是首个匹配位置。',
        score: 1 - index * 0.1,
        keywordScore: 1 - index * 0.1,
        vectorScore: null,
      })),
    };
  },
  // The seeded mock has one running and one partial job, so finished jobs —
  // the history list — would never render. Three fixture rows cover the
  // one-line layout, the warning dot and an older day group.
  listJobs: async (knowledgeBaseId?: string) => {
    const jobs = await mock.listJobs(knowledgeBaseId);
    const stamp = (hoursAgo: number) => new Date(Date.now() - hoursAgo * 3600_000).toISOString();
    const history = [
      { id: 'job-h-today', operationId: 'operation-h-today', knowledgeBaseId: 'local-product-docs', type: 'index', completedItems: 24, failedItems: 0, totalItems: 24, hours: 2 },
      { id: 'job-h-old', operationId: 'operation-h-old', knowledgeBaseId: 'local-research-notes', type: 'delete', completedItems: 3, failedItems: 0, totalItems: 3, hours: 80 },
      { id: 'job-h-week', operationId: 'operation-h-week', knowledgeBaseId: 'local-product-docs', type: 'rebuild', completedItems: 41, failedItems: 0, totalItems: 41, hours: 300 },
    ].map((item) => ({
      id: item.id, operationId: item.operationId, knowledgeBaseId: item.knowledgeBaseId, type: item.type,
      status: 'completed', stage: 'committing', progress: 100, lastSequence: 5, outcome: null,
      completedItems: item.completedItems, failedItems: item.failedItems, totalItems: item.totalItems,
      heartbeatAt: null, error: null, createdAt: stamp(item.hours + 1), updatedAt: stamp(item.hours),
    })) as any[];
    return [...jobs, ...history];
  },
  // Neither seeded model is installed, so the hero card would never render.
  // One ready default plus one mid-download row covers both layouts.
  listEmbeddingModels: async () => {
    const list = await mock.listEmbeddingModels();
    return list.map((model, index) => index === 0
      ? { ...model, status: 'ready', integrityStatus: 'verified', loadReady: true, isDefault: true }
      : { ...model, status: 'installing', download: { id: 'dl-' + model.id, status: 'running', progress: 46, downloadedBytes: Math.round(model.sizeBytes * 0.46), totalBytes: model.sizeBytes, currentFile: 'model.onnx', errorCode: null, errorMessage: null, updatedAt: Date.now() } });
  },
  // The Tauri gateway has native pickers, the mock does not. Without these the
  // dialogs fall back to the <input type=file> branch, so the host-picker
  // buttons the redesign added would never render in this check.
  pickImportFiles: async () => [
    { name: '产品说明.md', relativePath: '产品说明.md', mimeType: 'text/markdown', sizeBytes: 18432 },
    { name: '季度复盘.docx', relativePath: '归档/季度复盘.docx', mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', sizeBytes: 245760 },
  ],
  pickImportFolder: async () => [
    { name: '用户手册.pdf', relativePath: '手册/用户手册.pdf', mimeType: 'application/pdf', sizeBytes: 1048576 },
  ],
};
const view = (query.get('view') || 'list') as any;
// The task center opens without a knowledge base, otherwise it would filter
// down to the guide base, which has no seeded jobs.
const knowledgeBaseId = query.get('kb') || (view === 'jobs' ? undefined : 'fox-user-guide');
const documentId = query.get('doc') || undefined;
function KnowledgeCheck() {
  const [route, setRoute] = React.useState({ view, knowledgeBaseId, documentId });
  const [asked, setAsked] = React.useState('');
  return <div className="fox-local-kb-check-frame" style={{height:'100vh',display:'flex',flexDirection:'column',overflow:'auto',background:'var(--fox-canvas)',color:'var(--fox-text)'}}>
    <LocalKnowledgeWorkspace {...route} gateway={gateway}
      onNavigate={(nextView, nextBase, nextDocument) => setRoute({ view: nextView, knowledgeBaseId: nextBase, documentId: nextDocument })}
      onAskDocument={(payload) => setAsked(`预览回调：${payload.knowledgeBaseId} / ${payload.documentId} / ${payload.documentName}`)} />
    {asked && <output role="status">{asked}</output>}
  </div>;
}
const previewUrl = new URL(location.href);
previewUrl.searchParams.delete('narrow');
createRoot(document.getElementById('root')!).render(query.has('narrow')
  ? <iframe title="800px 窄窗检查" src={previewUrl.href} style={{ width: 800, height: '100vh', border: 0 }} />
  : <KnowledgeCheck />);
(window as any).__kbCase = { view, generation: crypto.randomUUID() };
