const task = await taskSpace(1);
const page = task.page('p1');
await page.click('@59');
console.log(await page.snapshot());
console.log(await task.finish({keep:['p1']}));