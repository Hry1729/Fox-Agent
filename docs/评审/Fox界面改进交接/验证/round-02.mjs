const fs = await import("node:fs/promises");
const task = await taskSpace(1);
const page = task.page("p1");
const dir = "D:/python/projects/Fox/Fox/docs/评审/Fox界面改进交接/验证/";
async function shot(name){
  try{const r=await page.cdp("Page.captureScreenshot",{format:"png",captureBeyondViewport:false});await fs.writeFile(dir+name,Buffer.from(r.data,"base64"));return name;}catch(e){return name+" FAILED "+e.message;}
}
const log=[];
await page.cdp("Emulation.setDeviceMetricsOverride",{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
await page.reload();
log.push(await shot("desktop-cancel.png"));
const r = await page.evaluate(async()=>{
  const out=[];const q=s=>document.querySelector(s);const wait=ms=>new Promise(r=>setTimeout(r,ms));
  const sel=(id,v)=>{const e=document.getElementById(id);e.value=v;e.dispatchEvent(new Event("change"));};
  const fb=()=>document.getElementById("feedback").textContent;
  sel("cancel-state","running");q("#cancel-demo").click();out.push(["cancel","->",q(".turn-summary").textContent]);
  q("#settle-demo").click();out.push(["settle","->",q(".turn-summary").textContent,fb()]);
  sel("cancel-state","empty");out.push(["empty",q(".output-note").textContent]);
  sel("cancel-state","warning");out.push(["warning",!!q(".notice")]);
  q("#send-demo").click();out.push(["send empty",fb()]);
  const d=q("#draft");d.value="测试草稿";d.dispatchEvent(new Event("input"));q("#send-demo").click();out.push(["send",fb()]);
  q('[data-scene=error]').click();out.push(["error draft kept",q("#draft").value]);
  q("#open-settings").click();out.push(["settings",document.getElementById("scene-title").textContent]);
  sel("test-outcome","failure");q("#test-connection").click();out.push(["testing",q("#test-connection").disabled]);await wait(800);out.push(["fail",q("#connection-result").textContent]);
  sel("test-outcome","success");q("#test-connection").click();await wait(800);out.push(["ok",q("#connection-result").textContent]);
  q("#clear-key").checked=true;q("#clear-key").dispatchEvent(new Event("change"));q("#provider-form").requestSubmit();out.push(["clear",q("#key-state").textContent]);
  q("#return-draft").click();out.push(["return",document.getElementById("scene-title").textContent,q("#draft").value]);
  q('[data-scene=composer]').click();sel("composer-state","new");out.push(["new",q("#before-label").textContent,!!q(".welcome-work")]);
  sel("agent-demo","Codex");out.push(["agent",fb()]);
  q('[data-scene=approval]').click();q("#deny-demo").click();out.push(["deny",fb()]);q("#reset-approval").click();q("#allow-demo").click();out.push(["allow",fb()]);q("#reset-approval").click();q("#cancel-approval").click();out.push(["cancelApproval",q(".decision").textContent]);
  for(const v of ["before","after","split"]){q(`[data-view=${v}]`).click();out.push(["view",v,getComputedStyle(q(".before")).display,getComputedStyle(q(".after")).display]);}
  q('[data-scene=cancel]').click();q("#zoom-evidence").click();out.push(["dialog open",q("#evidence-dialog").open,q("#dialog-image").naturalWidth]);
  return {out,errors:window.__prototypeErrors};
});
console.log(JSON.stringify(r,null,1));
await page.press("body","Escape");
console.log("dialog after Escape", await page.evaluate(()=>document.getElementById("evidence-dialog").open));
await page.focus("[data-scene=error]");await page.press("[data-scene=error]","Enter");
console.log("keyboard scene", await page.evaluate(()=>[document.getElementById("scene-title").textContent,getComputedStyle(document.activeElement).outlineStyle]));
log.push(await shot("desktop-error.png"));
for(const s of ["settings","approval","composer"]){await page.evaluate(x=>document.querySelector(`[data-scene=${x}]`).click(),s);log.push(await shot("desktop-"+s+".png"));}
await page.cdp("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:2,mobile:true});
const narrow=[];
for(const s of ["cancel","error","composer","approval","settings"]){
  await page.evaluate(x=>{document.querySelector(`[data-scene=${x}]`).click();window.scrollTo(0,0)},s);
  narrow.push(await page.evaluate(x=>{const bad=[...document.querySelectorAll("button,select,input,textarea")].filter(e=>e.offsetParent&&e.getBoundingClientRect().height<43&&e.type!=="checkbox").map(e=>e.textContent.trim()||e.id).slice(0,6);const over=[...document.querySelectorAll("body *")].filter(e=>e.getBoundingClientRect().right>innerWidth+1&&!e.closest("dialog")).map(e=>e.tagName+"."+e.className).slice(0,5);return {s:x,scrollW:document.documentElement.scrollWidth,small:bad,over};},s));
  await page.evaluate(()=>document.querySelector(".after").scrollIntoView());
  log.push(await shot("mobile-"+s+".png"));
}
console.log(JSON.stringify(narrow,null,1));
console.log(log);
