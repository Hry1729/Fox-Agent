const task = await taskSpace(1);
const page = task.page('p1');
await page.click('loc=role:button[name="03 项目菜单间距"]');
console.log(await page.snapshot());