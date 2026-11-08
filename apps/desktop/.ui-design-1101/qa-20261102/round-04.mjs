const task = await taskSpace(1);
const page = task.page('p1');
console.log(await page.evaluate(()=>{
  const menu=document.querySelector('.fox-composer-project-menu');
  const item=menu.querySelector('[cmdk-item]');
  const add=menu.querySelector('.fox-composer-project-add');
  const input=menu.querySelector('[data-slot="command-input-wrapper"]');
  const divider=menu.querySelector('[cmdk-separator]');
  return {padding:getComputedStyle(menu).padding,searchX:input.getBoundingClientRect().x,projectX:item.getBoundingClientRect().x,addX:add.getBoundingClientRect().x,projectIconX:item.querySelector('svg').getBoundingClientRect().x,addIconX:add.querySelector('svg').getBoundingClientRect().x,separatorMargin:getComputedStyle(divider).margin};
}));
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/03-after.png'})});
await page.keyboard.press('Escape');
await page.click('loc=role:button[name="05 交付文件卡片"]');
await page.click('text="当前实现"');
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/05-before.png'})});
await page.click('text="优化方案"');
await page.waitForSelector('.design-file-card');
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/05-after.png'})});
console.log(await page.snapshot());