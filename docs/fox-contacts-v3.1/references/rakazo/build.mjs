import fs from 'node:fs/promises';
import path from 'node:path';
import { createRequire } from 'node:module';
const root = path.dirname(new URL(import.meta.url).pathname.replace(/^\/(?=[A-Z]:)/i, ''));
const modules = 'D:/python/projects/Fox/worktrees/fox-contacts-m2-20261106/node_modules/.pnpm';
const buildRequire = createRequire(`${modules}/esbuild@0.25.12/node_modules/esbuild/package.json`);
const { build } = buildRequire('esbuild');
const { compile } = await import(`file:///${modules}/@tailwindcss+node@4.3.2/node_modules/@tailwindcss/node/dist/index.mjs`);
const oxideRequire = createRequire(`${modules}/@tailwindcss+oxide@4.3.2/node_modules/@tailwindcss/oxide/package.json`);
const { Scanner } = oxideRequire('@tailwindcss/oxide');
const read = name => fs.readFile(path.join(root,name),'utf8');
const result = await build({
  absWorkingDir:root, entryPoints:['entry.tsx'], bundle:true, minify:true, format:'iife', jsx:'automatic', write:false,
  define:{'process.env.NODE_ENV':'"production"'},
  plugins:[{name:'fixture-adapters',setup(b){
    b.onResolve({filter:/^@lingui\//},()=>({path:path.join(root,'compat.tsx')}));
    b.onResolve({filter:/^@rakazo\//},args=>({path:path.join(root,args.path==='@rakazo/ui-web/lib/utils'?'upstream/utils.ts':args.path==='@rakazo/ui-web'?'ui.tsx':'compat.tsx')}));
    b.onResolve({filter:/MessageHoverMetadata$/},()=>({path:path.join(root,'upstream/MessageHoverMetadata.tsx')}));
    b.onResolve({filter:/^\./},args=>{
      if(args.importer.endsWith('message-view.tsx')&&!args.path.endsWith('message-hover-actions')) return {path:path.join(root,'compat.tsx')};
    });
  }}]
});
const uiCss=(await read('upstream/styles.css')).replace(/^@import.*;\s*$/gm,'').replace(/^@source.*;\s*$/gm,'');
const cssInput=`@import "${modules}/tailwindcss@4.3.2/node_modules/tailwindcss/index.css";\n@import "${modules}/tw-animate-css@1.4.0/node_modules/tw-animate-css/dist/tw-animate.css";\n${await read('upstream/shadcn.css')}\n${await read('upstream/tokens.css')}\n${uiCss}\n${await read('upstream/markdown.web.css')}`;
const compiler=await compile(cssInput,{base:root,onDependency:()=>{}});
const scanner=new Scanner({sources:[{base:root,pattern:'entry.tsx',negated:false},{base:path.join(root,'upstream'),pattern:'*.tsx',negated:false}]});
const css=compiler.build(scanner.scan());
const wrapper=`html,body{margin:0;background:var(--background);color:var(--foreground);font-family:var(--font-sans);-webkit-font-smoothing:antialiased}.preview-page{max-width:960px;margin:0 auto;padding:30px 40px}.preview-header{display:flex;justify-content:space-between;align-items:center;gap:20px}.preview-header h1{margin:0;font-size:23px;font-weight:600}.preview-header p{margin:8px 0 0;color:var(--muted-foreground);font-size:13px}.preview-theme{border:1px solid var(--border);border-radius:8px;padding:8px 12px;font-size:13px;background:var(--background)}.preview-note{margin:24px 0 16px;font-size:13px;color:var(--muted-foreground);line-height:1.7}.preview-chat{border:1px solid var(--border);border-radius:14px;overflow:hidden}.preview-chat-title{padding:15px 28px;border-bottom:1px solid var(--border);font-size:15px;font-weight:600}.preview-chat-title span{font-size:12px;font-weight:400;color:var(--muted-foreground);margin-left:10px}.preview-help{font-size:13px;margin:18px 0;color:var(--muted-foreground)}footer{border-top:1px solid var(--border);padding-top:16px;margin-top:22px;font-size:11px;line-height:1.8;color:var(--faint-foreground)}footer a{text-decoration:underline;margin-left:8px}@media(max-width:600px){.preview-page{padding:22px 14px}.preview-header h1{font-size:19px}}`;
const html=`<!doctype html><html lang="zh-CN" data-theme="light"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Rakazo 消息组件预览</title><style>${css}\n${wrapper}</style></head><body><div id="root"></div><script>${result.outputFiles[0].text.replaceAll('</script','<\\/script')}</script></body></html>`;
await fs.writeFile(path.join(root,'index.html'),html,'utf8');
console.log({output:path.join(root,'index.html'),bytes:Buffer.byteLength(html)});
