const task = await taskSpace(1);
const page = task.page('p1');
await page.waitForSelector('.design-stage[data-surface="messages"]');
console.log(await page.evaluate(()=>{
  const stage=document.querySelector('.design-stage');
  const scene=document.querySelector('.design-message-scene');
  const process=document.querySelector('.design-process');
  const stopped=document.querySelector('.design-stopped > .aui-stopped-run');
  const text=process.querySelector('span');
  const state=stopped.querySelector('div>span');
  const r=stage.getBoundingClientRect(),s=scene.getBoundingClientRect();
  return {stagePadding:getComputedStyle(stage).padding,contentLeft:s.x-r.x,processX:process.getBoundingClientRect().x,stoppedX:stopped.getBoundingClientRect().x,processTextX:text.getBoundingClientRect().x,stateGrid:getComputedStyle(state).gridTemplateColumns,rowGap:stopped.getBoundingClientRect().top-process.getBoundingClientRect().bottom};
}));
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/04-after.png'})});
await page.click('loc=role:button[name="03 项目菜单间距"]');
await page.click('loc=role:button[name="测试项目"]');
await page.waitForFunction(()=>{const e=document.querySelector('.fox-composer-project-menu');return e && getComputedStyle(e).opacity==='1' && getComputedStyle(e).animationName==='none'});
console.log(await page.evaluate(()=>{
  const menu=document.querySelector('.fox-composer-project-menu');
  const item=menu.querySelector('[cmdk-item]');
  const add=menu.querySelector('.fox-composer-project-add');
  const input=menu.querySelector('[data-slot="command-input-wrapper"]');
  const divider=menu.querySelector('[cmdk-separator]');
  return {padding:getComputedStyle(menu).padding,searchX:input.getBoundingClientRect().x,projectX:item.getBoundingClientRect().x,addX:add.getBoundingClientRect().x,projectIconX:item.querySelector('svg').getBoundingClientRect().x,addIconX:add.querySelector('svg').getBoundingClientRect().x,separatorMargin:getComputedStyle(divider).margin};
}));
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/03-after.png'})});
console.log(await page.snapshot());