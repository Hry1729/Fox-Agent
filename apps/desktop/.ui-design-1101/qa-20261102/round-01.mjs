const task = await taskSpace('Fox UI refinement 20261102');
const page = task.page('p1');
await page.goto('http://127.0.0.1:1461/.ui-design-1101/?item=project');
await page.waitForSelector('loc=role:heading[name="项目菜单间距"]');
console.log({ spaceId: task.spaceId, page: page.label });
console.log(await page.snapshot());