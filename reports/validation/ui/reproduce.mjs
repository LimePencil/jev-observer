import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(scriptDir, '../../..');
const dir = path.resolve(process.env.JEV_VALIDATION_UI_DIR || path.join(root, '.jev-observer/independent-validation-rerun/ui'));
await mkdir(dir, {recursive: true});
const require = createRequire(path.join(root, 'ui/package.json'));
const { chromium } = require('@playwright/test');
const backendOrigin = 'http://127.0.0.1:19862';
const backend = spawn(path.resolve(process.env.JEV_VALIDATION_CURRENT || path.join(root, 'target/release/jev-observer')), ['--port','19862','--db',path.join(dir,'disposable.sqlite')], {cwd: dir, stdio:['ignore','pipe','pipe']});
let backendLog=''; backend.stdout.on('data',x=>backendLog+=x);backend.stderr.on('data',x=>backendLog+=x);
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const servers=[];
const results={createdAt:new Date().toISOString(),method:{before:'Production build of supplied before/dist (see reproduction prerequisites)',after:'Production build of supplied after/dist (see reproduction prerequisites)',backend:'Both snapshots proxy real API traffic to current release binary, private empty DB, port 19862',clipboard:'Default uses untouched browser permissions; allowed grants native clipboard read/write; denied uses CDP Browser.setPermission for clipboard-write; unavailable replaces navigator.clipboard with undefined at document init',delete:'Browser interception performs route.fetch() against real DELETE, then withholds its real response until user has opened another panel. No fabricated API data.'},clipboard:[],deletion:[]};
let browser;
async function serve(label,port) {
 const server=createServer(async(req,res)=>{
  try {
   if(req.url.startsWith('/api/')) {
    const chunks=[];for await (const chunk of req) chunks.push(chunk);
    const headers={...req.headers,host:'127.0.0.1:19862'};delete headers.connection;delete headers['content-length'];
    if(headers.origin===`http://127.0.0.1:${port}`)headers.origin=backendOrigin;
    const upstream=await fetch(backendOrigin+req.url,{method:req.method,headers,body:['GET','HEAD'].includes(req.method)?undefined:Buffer.concat(chunks)});
    res.writeHead(upstream.status,Object.fromEntries([...upstream.headers].filter(([k])=>!['content-encoding','transfer-encoding'].includes(k))));
    res.end(Buffer.from(await upstream.arrayBuffer()));return;
   }
   const pathname=new URL(req.url,`http://127.0.0.1:${port}`).pathname;
   const target=path.join(dir,label,'dist',pathname==='/'?'index.html':pathname);
   const ext=path.extname(target); const mime={'.html':'text/html','.js':'application/javascript','.css':'text/css','.woff2':'font/woff2','.png':'image/png','.svg':'image/svg+xml'}[ext]||'application/octet-stream';
   res.writeHead(200,{'content-type':mime});res.end(await readFile(target));
  } catch(error){res.writeHead(500);res.end(String(error));}
 });
 await new Promise(resolve=>server.listen(port,'127.0.0.1',resolve));servers.push(server);
}
async function setup(origin,mode) {
 const context=await browser.newContext({viewport:{width:1280,height:900},reducedMotion:'reduce'});
 if(mode==='allowed')await context.grantPermissions(['clipboard-read','clipboard-write'],{origin});
 if(mode==='unavailable')await context.addInitScript(()=>Object.defineProperty(navigator,'clipboard',{configurable:true,value:undefined}));
 const page=await context.newPage();
 const errors=[];page.on('pageerror',e=>errors.push({name:e.name,message:e.message}));
 await page.addInitScript(()=>{window.__rejections=[];window.addEventListener('unhandledrejection',event=>window.__rejections.push(String(event.reason)));});
 const cdp=await context.newCDPSession(page);
 if(mode==='denied'){
  const {targetInfo}=await cdp.send('Target.getTargetInfo');
  await cdp.send('Browser.setPermission',{permission:{name:'clipboard-write'},setting:'denied',origin,browserContextId:targetInfo.browserContextId});
 }
 await page.goto(origin,{waitUntil:'networkidle'});
 await page.getByRole('button',{name:'Open settings',exact:true}).waitFor();
 return {context,page,cdp,errors};
}
async function clipboard(label,origin,mode) {
 const {context,page,cdp,errors}=await setup(origin,mode);
 try {
  const initial=await page.evaluate(async()=>({secureContext:isSecureContext,clipboardAvailable:!!navigator.clipboard,writePermission:await navigator.permissions.query({name:'clipboard-write'}).then(p=>p.state).catch(e=>String(e))}));
  await page.getByRole('button',{name:'Connect an application',exact:true}).click();
  await page.getByRole('button',{name:'Copy local base URL',exact:true}).click();
  await page.waitForTimeout(250);
  const feedback=await page.evaluate(()=>[...document.querySelectorAll('[role="status"]')].filter(el=>/cop(y|ied)/i.test(el.textContent)).map(el=>({text:el.textContent,insideDialog:!!el.closest('[role="dialog"]'),ariaHiddenAncestors:[...function*(n){while(n){if(n.getAttribute('aria-hidden')==='true')yield n.tagName+(n.id?'#'+n.id:'');n=n.parentElement;}}(el)],display:getComputedStyle(el).display})));
  const ax=await cdp.send('Accessibility.getFullAXTree');
  const exposedCopyFeedback=ax.nodes.filter(n=>!n.ignored&&/Local base URL copied\.|Could not copy\. Select/.test(n.name?.value||'')).map(n=>({role:n.role?.value,name:n.name?.value}));
  const accessibleStatuses=await page.getByRole('status').allTextContents();
  await writeFile(path.join(dir,`${label}-clipboard-${mode}-ax.json`),JSON.stringify(ax,null,2));
  await page.screenshot({path:path.join(dir,`${label}-clipboard-${mode}.png`)});
  let readback=null;
  if(mode==='default'||mode==='allowed') {
   await context.grantPermissions(['clipboard-read'],{origin});
   readback=await page.evaluate(()=>navigator.clipboard.readText()).catch(e=>String(e));
  }
  results.clipboard.push({label,mode,initial,pageErrors:errors,unhandledRejections:await page.evaluate(()=>window.__rejections),feedback,accessibleStatuses,exposedCopyFeedback,readback});
 }finally{await context.close();}
}
async function deletion(label,origin,nextPanel) {
 const {context,page,errors}=await setup(origin,'default');
 let release,seen;
 const gate=new Promise(r=>release=r), arrived=new Promise(r=>seen=r);
 let responseStatus=null;let responseText=null;
 let responseSent;
 const sent=new Promise(r=>responseSent=r);
 await page.route('**/api/data',async route=>{
  const response=await route.fetch();responseStatus=response.status();responseText=await response.text();seen();
  await gate;await route.fulfill({response});responseSent();
 });
 try {
  await page.getByRole('button',{name:'Open settings',exact:true}).click();
  await page.getByRole('button',{name:'Delete history',exact:true}).click();
  await page.getByRole('textbox',{name:'Type DELETE to confirm'}).fill('DELETE');
  await page.getByRole('button',{name:'Permanently delete history',exact:true}).click();
  await arrived;
  const originalWasDeleting=await page.getByRole('button',{name:'Deleting…',exact:true}).isVisible();
  const beforeRelease={};
  if(nextPanel!=='stay'){
   await page.getByRole('button',{name:'Close detail panel',exact:true}).click();
   if(nextPanel==='connect') await page.getByRole('button',{name:'Connect an application',exact:true}).click();
   else await page.getByRole('button',{name:'Open settings',exact:true}).click();
  }
  beforeRelease.dialogs=await page.getByRole('dialog').count();
  beforeRelease.heading=await page.getByRole('dialog').getByRole('heading').allTextContents();
  await page.screenshot({path:path.join(dir,`${label}-delete-${nextPanel}-pending.png`)});
  release();await sent;
  await page.waitForTimeout(500);
  const afterRelease={dialogs:await page.getByRole('dialog').count(),heading:await page.getByRole('dialog').getByRole('heading').allTextContents(),toast:await page.locator('.toast').allTextContents()};
  await page.screenshot({path:path.join(dir,`${label}-delete-${nextPanel}-completed.png`)});
  results.deletion.push({label,nextPanel,originalWasDeleting,responseStatus,responseText,beforeRelease,afterRelease,pageErrors:errors});
 }finally{release();await context.close();}
}
try {
 let ready=false;for(let i=0;i<100;i++){try{if((await fetch(backendOrigin+'/api/health')).ok){ready=true;break;}}catch{}await sleep(50);}
 if(!ready)throw new Error('Backend did not start: '+backendLog);
 await serve('before',19861);await serve('after',19863);
 browser=await chromium.launch();
 results.browser=await browser.version();
 for(const [label,port] of [['before',19861],['after',19863]]) {
  const origin=`http://127.0.0.1:${port}`;
  for(const mode of ['default','allowed','denied','unavailable']) await clipboard(label,origin,mode);
  for(const next of ['stay','connect','settings']) await deletion(label,origin,next);
 }
 await writeFile(path.join(dir,'results.json'),JSON.stringify(results,null,2)+'\n');
 console.log(JSON.stringify(results,null,2));
} finally {
 if(browser)await browser.close();
 for(const server of servers)await new Promise(resolve=>server.close(resolve));
 backend.kill('SIGTERM');
 await writeFile(path.join(dir,'backend.log'),backendLog);
}
