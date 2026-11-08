const task = await claimTaskSpace(1);
const page = await task.adopt(task.userPage());
console.log({spaceId:task.spaceId,page:page.label});
console.log(await page.snapshot());