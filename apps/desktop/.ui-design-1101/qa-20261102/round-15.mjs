const task = await taskSpace(1);
const page = task.page('p1');
await page.waitForFunction(()=>getComputedStyle(document.querySelector('.fox-composer-draft-footer')).paddingLeft==='4px');
await page.hover('loc=css:.fox-composer-draft-footer .fox-composer-project-trigger');
console.log(await page.evaluate(()=>{const shell=document.querySelector('.fox-prompt-shell').getBoundingClientRect();const footer=document.querySelector('.fox-composer-draft-footer').getBoundingClientRect();return {footerHeight:footer.height,buttons:[...document.querySelectorAll('.fox-composer-draft-footer .fox-composer-project-trigger,.fox-composer-draft-footer .fox-composer-agent-trigger')].map(e=>{const r=e.getBoundingClientRect(),s=getComputedStyle(e);return {kind:e.className,height:r.height,leftGap:r.left-shell.left,rightGap:shell.right-r.right,bottomGap:shell.bottom-r.bottom,radius:s.borderRadius,background:s.backgroundColor}})}}));
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/01-project-hover.png'})});
await page.hover('loc=css:.fox-composer-draft-footer .fox-composer-agent-trigger');
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/01-agent-hover.png'})});
console.log(await page.snapshot());