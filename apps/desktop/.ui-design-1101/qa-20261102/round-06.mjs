const task = await taskSpace(1);
const page = task.page('p1');
console.log(await page.evaluate(()=>{const e=document.querySelector('.design-file-card'), s=getComputedStyle(e);return {background:s.backgroundColor,card:s.getPropertyValue('--card'),border:s.borderColor,color:s.color,muted:getComputedStyle(e.querySelector('.design-file-card-meta')).color,classes:e.className,viewport:[innerWidth,innerHeight]}}));
await page.hover('loc=css:.design-heading');
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/05-after-dark.png'})});
await page.click('loc=css:button[aria-label="切换浅色"]');
await page.click('loc=role:button[name="03 项目菜单间距"]');
console.log(await page.snapshot());