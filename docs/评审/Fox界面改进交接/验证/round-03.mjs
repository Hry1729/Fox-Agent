const task = await taskSpace(1);
const page = task.page("p1");
const fs = await import("node:fs/promises");
const dir = "D:/python/projects/Fox/Fox/docs/评审/Fox界面改进交接/验证/";
await page.cdp("Emulation.setDeviceMetricsOverride",{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
await page.reload();
let r=await page.cdp("Page.captureScreenshot",{format:"png"});await fs.writeFile(dir+"desktop-cancel-2.png",Buffer.from(r.data,"base64"));
console.log(await page.evaluate(()=>{const t=document.querySelector(".tool");return {toolH:t.getBoundingClientRect().height,scrollH:t.scrollHeight,errors:window.__prototypeErrors}}));
await page.cdp("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:2,mobile:true});
const res=[];
for(const s of ["cancel","composer","settings"]){await page.evaluate(x=>document.querySelector(`[data-scene=${x}]`).click(),s);res.push(await page.evaluate(()=>({w:document.documentElement.scrollWidth,small:[...document.querySelectorAll("button,select,input,textarea")].filter(e=>e.offsetParent&&e.getBoundingClientRect().height<43&&e.type!=="checkbox").length})));}
console.log(res);
await task.finish({keep:[]});
