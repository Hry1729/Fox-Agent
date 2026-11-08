const task = await taskSpace(1);
const page = task.page('p1');
console.log(await page.evaluate(()=>[...document.querySelectorAll('.review-message-thread .fox-thread-msg')].map(e=>({class:e.className,role:e.getAttribute('data-role')}))));
console.log(await page.snapshot());