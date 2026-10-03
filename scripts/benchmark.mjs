#!/usr/bin/env node
// Local-only load experiment. No provider calls, credentials or paid inference.
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import fs from 'node:fs/promises';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {performance} from 'node:perf_hooks';
import {createHash, randomBytes} from 'node:crypto';

const args = Object.fromEntries(process.argv.slice(2).map(a => {
  const argument=a.replace(/^--/,''),separator=argument.indexOf('=');
  return separator<0?[argument,true]:[argument.slice(0,separator),argument.slice(separator+1)];
}));
const rate = Number(args.rate ?? 500), seconds = Number(args.seconds ?? 60), delay = Number(args['upstream-ms'] ?? 25);
if (![rate,seconds,delay].every(Number.isFinite) || rate < 1 || seconds < 1 || delay < 0) throw new Error('Use positive --rate=N --seconds=N and nonnegative --upstream-ms=N');
const baselineSeconds = Number(args['baseline-seconds'] ?? Math.min(seconds,30));
if ((args['baseline-seconds'] !== undefined && typeof args['baseline-seconds'] !== 'string') || !Number.isFinite(baselineSeconds) || baselineSeconds <= 0 || Math.floor(baselineSeconds * rate) < 1) throw new Error('Use --baseline-seconds=N with a finite positive duration offering at least one request');
if (args['dashboard-search'] !== undefined && (typeof args['dashboard-search'] !== 'string' || Buffer.byteLength(args['dashboard-search']) > 4096)) throw new Error('Use --dashboard-search=TEXT with at most 4096 UTF-8 bytes');
const dashboardQuery = new URLSearchParams({window:'all'});
if (args['dashboard-search']) dashboardQuery.set('search',args['dashboard-search']);
const dashboardPath = `/api/dashboard?${dashboardQuery}`;
const binary = path.resolve(String(args.binary ?? 'target/release/jev-observer'));
if (args['keep-db'] && !process.env.JEV_OBSERVER_DB_KEY) throw new Error('--keep-db requires JEV_OBSERVER_DB_KEY so retained encrypted databases can be reopened');
const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) => !['typesafe_api_key', 'http_proxy', 'https_proxy', 'all_proxy'].includes(name.toLowerCase())));
environment.NO_PROXY = '127.0.0.1,localhost';
environment.JEV_OBSERVER_DB_KEY = process.env.JEV_OBSERVER_DB_KEY ?? randomBytes(32).toString('hex');
const workRoot = path.resolve(String(args['work-dir'] ?? '.jev-observer/benchmarks'));
await fs.mkdir(workRoot,{recursive:true});
const directory = await fs.mkdtemp(path.join(workRoot,'run-'));
const binarySha256 = createHash('sha256').update(await fs.readFile(binary)).digest('hex');
const requestAgent = new http.Agent({keepAlive:true,maxSockets:1024,maxFreeSockets:128});
const dashboardAgent = new http.Agent({keepAlive:true,maxSockets:2});
let proxy, peakRssKiB = 0, peakLagMs = 0, peakQueueDepth = 0, healthPollErrors = 0, running = true, mockCount = 0, stderr = '';
let dashboardAuthorization = '';
let observerAccess = '';
const percentile = (values,p) => values.length ? [...values].sort((a,b)=>a-b)[Math.max(0,Math.ceil(values.length*p)-1)] : null;
const sleep = ms => new Promise(r=>setTimeout(r,ms));
let previousHostCpu = null;
async function sampleHost() {
  const cpus = os.cpus();
  let cpuSource='os.cpus';
  let counters = cpus.reduce((sum,cpu) => ({total:sum.total+Object.values(cpu.times).reduce((a,b)=>a+b,0),idle:sum.idle+cpu.times.idle,iowait:0}),{total:0,idle:0,iowait:0});
  const sample = {at:new Date().toISOString(),load_average:os.loadavg(),free_memory_mib:os.freemem()/2**20,available_memory_mib:null,total_memory_mib:os.totalmem()/2**20,swap_total_mib:null,swap_free_mib:null,swap_used_mib:null,runnable_tasks:null,cpu_idle_percent:null,cpu_iowait_percent:null,cpu_busy_percent:null,cpu_sample_interval_ms:null};
  if (process.platform === 'linux') {
    const [memory,stat] = await Promise.allSettled([fs.readFile('/proc/meminfo','utf8'),fs.readFile('/proc/stat','utf8')]);
    if (memory.status === 'fulfilled') {
      const kib = key => {const match=memory.value.match(new RegExp(`^${key}:\\s+(\\d+)\\s+kB$`,'m'));return match?Number(match[1]):null};
      const available=kib('MemAvailable'),total=kib('SwapTotal'),free=kib('SwapFree');
      sample.available_memory_mib=available===null?null:available/1024;
      sample.swap_total_mib=total===null?null:total/1024;
      sample.swap_free_mib=free===null?null:free/1024;
      sample.swap_used_mib=total===null||free===null?null:(total-free)/1024;
    } else sample.memory_sampling_error=memory.reason.code??'unavailable';
    if (stat.status === 'fulfilled') {
      // Exclude guest columns because Linux already includes them in user/nice.
      const line=stat.value.match(/^cpu\s+(.+)$/m), runnable=stat.value.match(/^procs_running\s+(\d+)$/m);
      if (line) {const times=line[1].trim().split(/\s+/).slice(0,8).map(Number);counters={total:times.reduce((a,b)=>a+b,0),idle:times[3],iowait:times[4]??0};cpuSource='linux.proc.stat';}
      sample.runnable_tasks=runnable?Number(runnable[1]):null;
    } else sample.cpu_sampling_error=stat.reason.code??'unavailable';
  }
  const now=performance.now(),previous=previousHostCpu;
  if (previous) {
    sample.cpu_sample_interval_ms=now-previous.at;
    const total=counters.total-previous.total;
    if (previous.source===cpuSource&&sample.cpu_sample_interval_ms>=250&&total>0) {
      sample.cpu_idle_percent=Math.max(0,Math.min(100,(counters.idle-previous.idle)/total*100));
      sample.cpu_iowait_percent=Math.max(0,Math.min(100,(counters.iowait-previous.iowait)/total*100));
      sample.cpu_busy_percent=Math.max(0,100-sample.cpu_idle_percent-sample.cpu_iowait_percent);
    }
  }
  sample.cpu_counter_source=cpuSource;
  previousHostCpu={...counters,at:now,source:cpuSource};
  return sample;
}
const hostStart=await sampleHost();
const small = {model:'jev-benchmark',state:{text:'A customer needs a billing correction. '.repeat(240)},questions:{
  department:{type:'choice',instructions:'Which team should handle this?',criteria:{billing:'Payments',technical:'Software',sales:'New customers'}},
  urgency:{type:'noul',instructions:'Is this time-sensitive?'},
  quality:{type:'score',instructions:'How complete is the report?',criteria:['Incomplete','Usable','Complete']}
}};
const large = structuredClone(small); large.state.text='Large synthetic document. '.repeat(5000);
for(let i=0;i<17;i++) large.questions[`check_${i}`]={type:'noul',instructions:`Does the document satisfy check ${i}?`};
const bodies = [Buffer.from(JSON.stringify(small)),Buffer.from(JSON.stringify(large))];
function responseFor(payload) {
  const answers={}; for(const [key,q] of Object.entries(payload.questions??{})) {
      answers[key]=q.type==='choice'?{type:'choice',choice:'billing',probabilities:{billing:0.8,technical:0.15,sales:0.05},confidence:0.65}:q.type==='score'?{type:'score',score:1.1,probabilities:{'0':0.1,'1':0.7,'2':0.2},legend:{'0':'Incomplete','1':'Usable','2':'Complete'},confidence:0.44}:{type:'noul',noul:0.84};
    }
  return JSON.stringify({model:'jev-benchmark-1',answers,usage:{input_tokens:1000,output_tokens:40},provider_extension:{preserve:true}});
}
const expectedResponses = [responseFor(small),responseFor(large)];
const mock = http.createServer((req,res) => {
  const chunks=[]; req.on('data',c=>chunks.push(c)); req.on('end',()=>{
    mockCount++;
    let payload; try {payload=JSON.parse(Buffer.concat(chunks));}catch{res.writeHead(400);res.end('bad fixture');return;}
    const response=responseFor(payload);
    setTimeout(()=>{res.writeHead(200,{'content-type':'application/json','content-length':Buffer.byteLength(response)});res.end(response);},delay);
  });
});
// Keep the deterministic fixture's idle sockets alive for the whole experiment.
// Transport-close behavior is covered separately by proxy integration tests.
mock.keepAliveTimeout = Math.max(600_000, (seconds + 120) * 1000);
await new Promise(r=>mock.listen(0,'127.0.0.1',r));
const mockPort=mock.address().port;
const portProbe=net.createServer(); await new Promise(r=>portProbe.listen(0,'127.0.0.1',r)); const proxyPort=portProbe.address().port; await new Promise(r=>portProbe.close(r));

function exchange(port,route,body,agent=requestAgent){
  return new Promise(resolve=>{
    const start=performance.now();let firstByte=null;
    const req=http.request({hostname:'127.0.0.1',port,path:route,method:body?'POST':'GET',agent,headers:body?{'authorization':'Bearer benchmark-fixture','content-type':'application/json','content-length':body.length,'x-observer-source':'benchmark','x-observer-access':observerAccess}:{authorization:dashboardAuthorization}},res=>{
      const chunks=[];res.on('data',chunk=>{firstByte??=performance.now()-start;chunks.push(chunk)});
      res.on('end',()=>resolve({status:res.statusCode,ms:performance.now()-start,firstByte,body:Buffer.concat(chunks).toString()}));
      res.on('error',error=>resolve({status:0,error:error.code??error.message,ms:performance.now()-start,body:''}));
    });
    req.setTimeout(30_000,()=>req.destroy(new Error('client timeout')));
    req.on('error',error=>resolve({status:0,error:error.code??error.message,ms:performance.now()-start,body:''}));
    if(body)req.end(body);else req.end();
  });
}

async function load(label,port,duration,targetRate,dashboard){
  const phaseHostStart=await sampleHost(),hostSamples=[];
  let hostSamplePending=null;
  const hostTimer=setInterval(()=>{
    if(hostSamplePending)return;
    hostSamplePending=sampleHost().then(sample=>hostSamples.push(sample)).catch(error=>hostSamples.push({at:new Date().toISOString(),sampling_error:error.code??error.message})).finally(()=>{hostSamplePending=null});
  },1000);
  hostTimer.unref();
  const start=performance.now(), total=Math.floor(duration*targetRate), durations=[],firstBytes=[],lateness=[];
  const warmupSeconds = Math.min(5,duration/2);
  let submitted=0,completed=0,steadyCompleted=0,errors=0,answerCount=0,dashboardStop=false,elapsed=0;
  const inFlight=new Set(), dashboardLatencies=[], dashboardErrors=[], failureDetails=[];
  const poll=(async()=>{while(!dashboardStop&&dashboard){const result=await exchange(proxyPort,dashboardPath,null,dashboardAgent);dashboardLatencies.push(result.ms);if(result.status!==200)dashboardErrors.push(result.status);await sleep(1000);}})();
  try { while(submitted<total){
    const elapsed=performance.now()-start;
    const due=Math.min(total,Math.floor(elapsed*targetRate/1000)+1);
    while(submitted<due){
      const index=submitted++, big=index%10===0;
      lateness.push(Math.max(0,performance.now()-start-index*1000/targetRate));
      answerCount+=big?20:3;
      const work=exchange(port,'/v1/systemone',bodies[big?1:0]).then(result=>{
        completed++;const finishedSeconds=(performance.now()-start)/1000;if(finishedSeconds>=warmupSeconds&&finishedSeconds<=duration)steadyCompleted++;durations.push(result.ms);if(result.firstByte!==null)firstBytes.push(result.firstByte);
        if(result.status!==200 || result.body!==expectedResponses[big?1:0]) {
          errors++; if(failureDetails.length<10)failureDetails.push({index,big,status:result.status,error:result.error??'Response body mismatch',body_bytes:Buffer.byteLength(result.body),expected_bytes:Buffer.byteLength(expectedResponses[big?1:0])});
        }
      }).finally(()=>inFlight.delete(work));inFlight.add(work);
    }
    if(inFlight.size>10000)throw new Error('Load generator exceeded 10,000 in-flight requests; test cannot claim the offered rate');
    await sleep(1);
  }
  await Promise.all(inFlight);
  elapsed=(performance.now()-start)/1000;
  } finally {clearInterval(hostTimer);if(hostSamplePending)await hostSamplePending;}
  dashboardStop=true;await poll;
  const phaseHostEnd=await sampleHost();
  const report={label,target_rps:targetRate,offered:total,completed,errors,failure_details:failureDetails,expected_answers:answerCount,load_duration_seconds:duration,warmup_seconds:warmupSeconds,elapsed_seconds:elapsed,achieved_rps:completed/elapsed,steady_completed:steadyCompleted,steady_rps:steadyCompleted/(duration-warmupSeconds),
    latency_ms:{p50:percentile(durations,.5),p95:percentile(durations,.95),p99:percentile(durations,.99),max:durations.reduce((maximum,value)=>Math.max(maximum,value),0)},
    first_byte_ms:{p50:percentile(firstBytes,.5),p95:percentile(firstBytes,.95),p99:percentile(firstBytes,.99)},
    scheduling_lateness_ms:{p95:percentile(lateness,.95),p99:percentile(lateness,.99)},
    dashboard:{queries:dashboardLatencies.length,errors:dashboardErrors,p95_ms:percentile(dashboardLatencies,.95)},
    host_telemetry:{start:phaseHostStart,end:phaseHostEnd,sample_interval_ms:1000,samples:hostSamples},
    collection_health_at_phase_end:dashboard?JSON.parse((await exchange(proxyPort,'/api/health',null,dashboardAgent)).body):null};
  process.stdout.write(JSON.stringify({...report,host_telemetry:{start:phaseHostStart,end:phaseHostEnd,samples_recorded:hostSamples.length}})+'\n');return report;
}

try{
  proxy=spawn(binary,['--port',String(proxyPort),'--db',path.join(directory,'bench.sqlite'),'--upstream',`http://127.0.0.1:${mockPort}/v1/systemone`,'--max-records','2000000'],{stdio:['ignore','ignore','pipe'],env:environment});
  proxy.stderr.on('data',chunk=>{stderr+=chunk.toString();if(stderr.length>16000)stderr=stderr.slice(-16000)});
  proxy.on('error',error=>{stderr+=error.message});
  for(let n=0;n<100;n++){try{const token=await fs.readFile(path.join(directory,'bench.access-token'),'utf8');observerAccess=token;dashboardAuthorization=`Basic ${Buffer.from(`observer:${token}`).toString('base64')}`;}catch{}if(dashboardAuthorization&&(await exchange(proxyPort,'/api/health',null,dashboardAgent)).status===200)break;if(proxy.exitCode!==null)throw new Error(stderr);await sleep(100);if(n===99)throw new Error('Observer did not start: '+stderr);}
  const memoryPoll=(async()=>{while(running){try{const status=await fs.readFile(`/proc/${proxy.pid}/status`,'utf8');peakRssKiB=Math.max(peakRssKiB,Number(status.match(/^VmRSS:\s+(\d+)/m)?.[1]??0));}catch{}
      try { const result = await exchange(proxyPort,'/api/health',null,dashboardAgent); const health = JSON.parse(result.body); if(result.status!==200)healthPollErrors++; peakLagMs=Math.max(peakLagMs,health.lag_ms??0); peakQueueDepth=Math.max(peakQueueDepth,health.queue_depth??0); } catch {healthPollErrors++;}
      await sleep(1000)}})();
  console.log(JSON.stringify({observer_origin:`http://127.0.0.1:${proxyPort}`,database_directory:directory,binary_sha256:binarySha256}));
  const settings=JSON.parse((await exchange(proxyPort,'/api/settings',null,dashboardAgent)).body);
  const baseline=await load('direct mock baseline',mockPort,baselineSeconds,rate,false);
  const sustained=await load('proxy with capture and dashboard',proxyPort,seconds,rate,true);
  const burst=args['no-burst']?null:await load('proxy burst with capture and dashboard',proxyPort,10,rate*2,true);
  let health;
  for(let n=0;n<300;n++){health=JSON.parse((await exchange(proxyPort,'/api/health',null,dashboardAgent)).body);if(health.queue_depth===0&&health.active_captures===0)break;await sleep(100);}
  const dashboard=JSON.parse((await exchange(proxyPort,'/api/dashboard?window=all',null,dashboardAgent)).body);
  const expected=sustained.completed+(burst?.completed??0),expectedAnswers=sustained.expected_answers+(burst?.expected_answers??0);
  const countsComplete=health.persisted===expected&&health.dropped===0&&health.truncated===0&&health.write_failures===0&&dashboard.summary.request_count===expected&&dashboard.summary.answer_count===expectedAnswers&&sustained.errors===0&&(burst?.errors??0)===0;
  const accountingComplete = dashboard.summary.input_tokens===expected*1000 && dashboard.summary.output_tokens===expected*40 && dashboard.groups.length===20 && dashboard.groups.every(group=>group.valid_count===group.answer_count) && mockCount===baseline.completed+expected && health.forwarded===expected;
  const throughputPassed = [sustained, ...(burst?[burst]:[])].every(phase=>phase.steady_rps>=phase.target_rps*.98&&phase.scheduling_lateness_ms.p99<=100&&phase.dashboard.errors.length===0);
  const databaseFile = await fs.open(path.join(directory,'bench.sqlite'),'r');
  const databaseHeader = Buffer.alloc(16);
  try { await databaseFile.read(databaseHeader,0,16,0); } finally { await databaseFile.close(); }
  const encrypted = !databaseHeader.equals(Buffer.from('SQLite format 3\0'));
  const complete = countsComplete && accountingComplete && throughputPassed && healthPollErrors === 0 && baseline.errors === 0 && encrypted;
  const hostEnd=await sampleHost();
  const report={date:new Date().toISOString(),binary,binary_sha256:binarySha256,database_directory:directory,node:process.version,platform:`${os.platform()} ${os.release()} ${os.arch()}`,cpu:os.cpus()[0]?.model,logical_cpus:os.cpus().length,total_memory_gib:os.totalmem()/2**30,
    dashboard_query:dashboardPath,host_telemetry:{start:hostStart,end:hostEnd,sample_interval_ms:1000,cpu_scope:'Whole host across all logical CPUs, including unrelated processes; CPU busy excludes idle and iowait. Short/initial sample intervals remain null.'},
    settings,storage:{mode:'sqlcipher',plaintext_header_absent:encrypted},fixture:{small_bytes:bodies[0].length,large_bytes:bodies[1].length,large_fraction:.1,questions:[3,20],upstream_delay_ms:delay},baseline,sustained,burst,peak_rss_mib:peakRssKiB/1024,peak_sampled_lag_ms:peakLagMs,peak_sampled_queue_depth:peakQueueDepth,health_poll_errors:healthPollErrors,database_bytes:(await fs.stat(path.join(directory,'bench.sqlite'))).size,health,summary:dashboard.summary,counts_complete:countsComplete,accounting_complete:accountingComplete,upstream_attempts:mockCount,throughput_passed:throughputPassed,complete,notes:['Local mock only; does not measure provider latency or paid API capacity.','Steady RPS excludes the stated warm-up and post-load drain; achieved_rps includes the drain. RSS and collection lag are sampled once per second.','Exact body/status and retained-count checks enabled.','Latency distributions are independent runs; subtracting percentiles does not produce a per-request overhead percentile.','The proxy, mock and generator share this host with unrelated processes. Phase host telemetry samples total CPU/load/memory pressure; it cannot attribute a failure to Observer or another workload by itself.','Runtime-reported free memory and Linux MemAvailable are recorded separately and may be equal; swap and runnable tasks are null on unsupported platforms. One-second host samples can miss short pressure spikes.']};
  const destination=path.resolve(String(args.output??'reports/benchmarks/latest.json'));await fs.mkdir(path.dirname(destination),{recursive:true});await fs.writeFile(destination,JSON.stringify(report,null,2)+'\n');
  console.log(`Saved ${destination}; complete=${complete}; peak RSS=${report.peak_rss_mib.toFixed(1)} MiB`);
  if(!complete)process.exitCode=1;
  running=false;await memoryPoll;
}finally{
  running=false;requestAgent.destroy();dashboardAgent.destroy();mock.closeAllConnections();await new Promise(r=>mock.close(r));
  if(proxy&&proxy.exitCode===null){proxy.kill('SIGTERM');await Promise.race([new Promise(r=>proxy.once('exit',r)),sleep(10000)]);if(proxy.exitCode===null)proxy.kill('SIGKILL');}
  if(args['keep-db']) console.log('Database retained at '+directory);
  else await fs.rm(directory,{recursive:true,force:true});
}
