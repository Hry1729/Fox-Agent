const task = await taskSpace(1);
const page = task.page('p1');
await page.click('loc=role:button[name="04 取消状态与消息操作"]');
await page.waitForSelector('.aui-stopped-run svg');
console.log(await page.evaluate(()=>{const stage=document.querySelector('.design-stage').getBoundingClientRect();const scene=document.querySelector('.design-message-scene').getBoundingClientRect();const icon=document.querySelector('.aui-stopped-run svg');const processText=document.querySelector('.design-process span').getBoundingClientRect();const stoppedText=document.querySelector('.aui-stopped-run > div > span');const range=document.createRange();range.selectNodeContents(stoppedText.lastChild);return {centerOffset:(scene.x+scene.width/2)-(stage.x+stage.width/2),icon:icon.getAttribute('class'),iconSize:[icon.getBoundingClientRect().width,icon.getBoundingClientRect().height],thoughtTextX:processText.x,cancelTextX:range.getBoundingClientRect().x}}));
console.log({screenshot:await page.screenshot({path:'D:/python/projects/Fox/Fox/apps/desktop/.ui-design-1101/qa-20261102/04-cancel-icon.png'})});
console.log(await page.snapshot());