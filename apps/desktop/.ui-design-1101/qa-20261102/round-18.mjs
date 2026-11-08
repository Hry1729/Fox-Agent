const task = await taskSpace(1);
const page = task.page('p1');
await page.waitForSelector('.fox-composer-draft-footer');
await page.hover('loc=css:.fox-composer-draft-footer .fox-composer-project-trigger');
console.log(await page.evaluate(()=>{const shell=document.querySelector('.fox-prompt-shell').getBoundingClientRect();const footer=document.querySelector('.fox-composer-draft-footer').getBoundingClientRect();return {footerHeight:footer.height,inputHeight:document.querySelector('.fox-prompt-input').getBoundingClientRect().height,buttons:[...document.querySelectorAll('.fox-composer-draft-footer .fox-composer-project-trigger,.fox-composer-draft-footer .fox-composer-agent-trigger')].map(e=>{const r=e.getBoundingClientRect();return {class:e.className,height:r.height,leftGap:r.left-shell.left,rightGap:shell.right-r.right,bottomGap:shell.bottom-r.bottom}})}}));
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/01-compact-footer.png'})});
console.log(await page.snapshot());