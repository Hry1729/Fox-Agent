const task = await taskSpace(1);
const page = task.page('p1');
await page.click('@43');
console.log(await page.snapshot());