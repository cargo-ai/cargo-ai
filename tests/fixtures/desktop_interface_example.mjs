// Exercise the shipped consumer with a bounded host double, without a DOM framework.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import vm from 'node:vm';
const nativeScript = process.env.CARGO_AI_DESKTOP_BRIDGE_SCRIPT ? await readFile(process.env.CARGO_AI_DESKTOP_BRIDGE_SCRIPT, 'utf8') : null;
const context = vm.createContext({TextEncoder, setTimeout});
vm.runInContext(await readFile(process.argv[2], 'utf8'), context);
const Preview = context.DesktopPreview;
const caps = () => ({version:2, epoch:'epoch', features:['business_inputs.v1','structured_results.v1','terminal_acknowledgment.v1','action_wait.v1'], inputBytesByAction:{'preview-note':12000}, handles:0, handleLimit:128, busy:false, reconciliationRequired:false});
const fault = code => Object.assign(Error(code), {code});
function host() {
  const h = {version:2, submitted:[], acknowledgments:[], canceled:[], reads:0,
    capabilities:async()=>caps(), connect:async()=>[], newSubmissionID:async()=>'token',
    submit:async request=>{h.submitted.push(request);return 'handle';},
    status:async()=>{h.reads++;return h.snapshot();},
    wait:async(handle, options)=>{assert.equal(handle,'handle');assert.equal(options.timeoutMs,1000);return h.status();},
    acknowledge:async handle=>{h.acknowledgments.push(handle);}, cancel:async handle=>{h.canceled.push(handle);},
    snapshot:()=>({handle:'handle',action:'preview-note',submissionID:'token',epoch:'epoch',state:'succeeded',canAcknowledge:true,
      terminal:{outcome:'succeeded',data:{client_action:{interface:'notes',action:'preview-note'},result:{availability:'available',content_included:true,content:{status:'previewed',text:'hello <b>world</b>'}}}}})};
  return h;
}
function exposed(h) {
  if (!nativeScript || h.version !== 2) return h;
  const sandbox = {TextEncoder, setTimeout}; sandbox.window = sandbox; sandbox.top = sandbox;
  sandbox.webkit = {messageHandlers:{cargoAIClientActions:{postMessage:async body=>{
    assert.equal(Object.keys(body).sort().join(','),'documentID,method,payload');
    assert.equal(typeof body.documentID,'string');assert(body.documentID.length);
    const p=body.payload;
    const calls={capabilities:()=>h.capabilities(),connect:()=>h.connect(),submissionID:()=>h.newSubmissionID(),
      submit:()=>h.submit(p),status:()=>h.status(p.handle),wait:()=>h.wait(p.handle,{timeoutMs:p.timeoutMs}),
      cancel:()=>h.cancel(p.handle),acknowledge:()=>h.acknowledge(p.handle)};
    assert(calls[body.method], 'unexpected native dispatch: '+body.method);
    try {return JSON.stringify({ok:true,value:await calls[body.method]()});}
    catch(error){return JSON.stringify({ok:false,error:{code:error.code||'host.unavailable',message:error.message}});}
  }}}};
  vm.runInContext(nativeScript,vm.createContext(sandbox));
  return sandbox.cargoAICompanion.actions;
}
async function client(h=host()) {const c=new Preview(exposed(h),()=>{},async()=>{});await c.connect();c.edit('hello <b>world</b>');return c;}
const goodHost=host(), good=await client(goodHost);await good.preview();
assert.equal(good.previewText, 'hello <b>world</b>');assert.equal(good.pending,null);
assert.equal(goodHost.submitted.length,1);assert.equal(goodHost.submitted[0].inputs.text,good.draft);
assert.equal(goodHost.acknowledgments.length,1);
// No fallback to a different model or action exists; fallback here is status observation only.
const polling=host();polling.capabilities=async()=>({...caps(),features:caps().features.filter(f=>f!=='action_wait.v1')});polling.wait=()=>{throw Error('wait not advertised');};
let polls=0;const settled=polling.snapshot;polling.snapshot=()=>++polls===1?{...settled(),state:'running',canAcknowledge:false,terminal:null}:settled();
const pc=await client(polling);await pc.preview();assert.equal(polls,2);assert.equal(pc.pending,null);
for(const key of ['handle','action','submissionID','epoch']) {
  const h=host(), snapshot=h.snapshot;h.snapshot=()=>({...snapshot(),[key]:'wrong'});
  const c=await client(h);await c.preview();assert(c.pending);assert.equal(c.previewText,'');assert.match(c.message,/does not match/);assert.equal(h.acknowledgments.length,0);
}
for(const key of ['interface','action']) {
  const h=host(), snapshot=h.snapshot;h.snapshot=()=>{const s=snapshot();s.terminal.data.client_action[key]='wrong';return s;};
  const c=await client(h);await c.preview();assert(c.pending);assert.equal(c.previewText,'');assert.equal(h.acknowledgments.length,0);assert.match(c.message,/another interface or action/);
}
const changedFeatures=host(), fc=await client(changedFeatures);changedFeatures.capabilities=async()=>({...caps(),features:[]});await fc.preview();assert.equal(changedFeatures.submitted.length,0);assert.match(fc.message,/lacks the required/);
const editedHost=host(), edited=await client(editedHost);editedHost.wait=async()=>{edited.edit('newer draft');return editedHost.snapshot();};await edited.preview();
assert.equal(edited.draft,'newer draft');assert.equal(edited.previewText,'');assert.match(edited.message,/older result/);assert.equal(editedHost.acknowledgments.length,1);
const mintHost=host(), mint=await client(mintHost);mintHost.newSubmissionID=async()=>{mint.edit('edited while minting');return 'token';};await mint.preview();
assert.equal(mintHost.submitted[0].inputs.text,'hello <b>world</b>');assert.equal(mint.previewText,'');assert.match(mint.message,/older result/);
const absent=host(), absentSnapshot=absent.snapshot;absent.snapshot=()=>{const s=absentSnapshot();delete s.terminal.data.client_action;return s;};const absentClient=await client(absent);await absentClient.preview();assert(absentClient.pending);assert.equal(absentClient.previewText,'');
const warning=host(), warningSnapshot=warning.snapshot;warning.snapshot=()=>{const s=warningSnapshot();s.terminal.warnings=[{code:'example.warning',message:'Example warning'}];return s;};const warningClient=await client(warning);await warningClient.preview();assert.match(warningClient.message,/reported warnings/);
const missing=new Preview(undefined);await missing.connect();assert.equal(missing.caps,null);assert.match(missing.message,/Open this project/);
const old=host();old.version=1;const oldClient=await client(old);await oldClient.preview();assert.equal(old.submitted.length,0);
const denied=host();denied.capabilities=async()=>{throw fault('host.authorization_required');};const dc=await client(denied);await dc.preview();assert.equal(dc.draft,'hello <b>world</b>');assert.equal(denied.submitted.length,0);assert.match(dc.message,/authorization_required/);
const noAction=host();noAction.capabilities=async()=>({...caps(),inputBytesByAction:{}});const nc=await client(noAction);await nc.preview();assert.equal(noAction.submitted.length,0);
for(const features of [[],['business_inputs.v1','structured_results.v1']]) {const h=host();h.capabilities=async()=>({...caps(),features});const c=await client(h);await c.preview();assert.equal(h.submitted.length,0);}
const hidden=host(), hiddenSnapshot=hidden.snapshot;hidden.snapshot=()=>{const s=hiddenSnapshot();s.terminal.data.result.content_included=false;delete s.terminal.data.result.content;return s;};const hc=await client(hidden);await hc.preview();assert.equal(hc.previewText,'');assert.match(hc.message,/unavailable/);assert.equal(hidden.submitted.length,1);assert.equal(hidden.acknowledgments.length,1);
for(const kind of ['business','terminal']) {const h=host(), snap=h.snapshot;h.snapshot=()=>{const s=snap();if(kind==='business')s.terminal.data.result.content.status='invalid';else{s.state='failed';s.terminal.outcome='failed';}return s;};const c=await client(h);await c.preview();assert.equal(c.previewText,'');assert.equal(h.acknowledgments.length,1);assert.equal(h.submitted.length,1);}
// Uncertain admission keeps the original intent and blocks a second submission.
const uncertain=host();uncertain.submit=async request=>{uncertain.submitted.push(request);throw Error('reply lost');};const uc=await client(uncertain);await uc.preview();await uc.preview();await uc.connect();assert.equal(uncertain.submitted.length,1);assert.equal(uc.pending.request.submissionID,'token');assert.match(uc.message,/do not repeat/);await uc.cancel();assert.match(uc.message,/cancellation cannot be addressed/);assert.equal(uncertain.canceled.length,0);
const ack=host();ack.acknowledge=async()=>{throw Error('cleanup reply lost');};const ac=await client(ack);await ac.preview();await ac.preview();assert.equal(ac.previewText,'hello <b>world</b>');assert(ac.pending);assert.equal(ack.submitted.length,1);assert.match(ac.message,/cleanup was not confirmed/);
const rejected=host();rejected.submit=async()=>{throw fault('host.permission_denied');};const rc=await client(rejected);await rc.preview();assert.equal(rc.pending,null);assert.equal(rc.draft,'hello <b>world</b>');
// Cancellation is requested while submit is unresolved, then observed through quiescence.
const cancelHost=host();let releaseSubmit;cancelHost.submit=request=>{cancelHost.submitted.push(request);return new Promise(resolve=>{releaseSubmit=resolve;});};
let phase=0;const cs=cancelHost.snapshot;cancelHost.snapshot=()=>{const s=cs();s.state='canceled';s.terminal.outcome='canceled';s.canAcknowledge=++phase>1;return s;};
const cc=await client(cancelHost);const running=cc.preview();while(!releaseSubmit)await new Promise(resolve=>setImmediate(resolve));await cc.cancel();releaseSubmit('handle');await running;
assert.equal(cancelHost.canceled.length,1);assert.equal(cancelHost.acknowledgments.length,1);assert.equal(phase,2);assert.equal(cc.previewText,'');assert.equal(cc.draft,'hello <b>world</b>');assert.match(cc.message,/Stopped/);
// Exercise the page's real event wiring and button projection while submit is pending.
const domHost=host();let resolveDomSubmit;
domHost.submit=request=>{domHost.submitted.push(request);return new Promise(resolve=>{resolveDomSubmit=resolve;});};
const domSnapshot=domHost.snapshot;domHost.snapshot=()=>{const s=domSnapshot();s.state='canceled';s.terminal.outcome='canceled';return s;};
const elements=Object.fromEntries(['draft','result','connect','preview','cancel','status'].map(id=>[id,{value:'',textContent:'',disabled:['preview','cancel'].includes(id),events:{},addEventListener(name,fn){this.events[name]=fn;}}]));
const domContext=vm.createContext({TextEncoder,setTimeout,cargoAICompanion:{actions:exposed(domHost)},document:{querySelector:selector=>elements[selector.slice(1)]}});
vm.runInContext(await readFile(process.argv[2],'utf8'),domContext);
await elements.connect.events.click();elements.draft.value='hello <b>world</b>';elements.draft.events.input();
const domRunning=elements.preview.events.click();while(!resolveDomSubmit)await new Promise(resolve=>setImmediate(resolve));
assert.equal(elements.cancel.disabled,false);assert.equal(elements.preview.disabled,true);
await elements.cancel.events.click();resolveDomSubmit('handle');await domRunning;
assert.equal(domHost.canceled.length,1);assert.equal(domHost.acknowledgments.length,1);assert.equal(elements.cancel.disabled,true);assert.equal(elements.preview.disabled,false);assert.match(elements.status.textContent,/Stopped/);
const unknown=host();unknown.snapshot=()=>({handle:'handle',action:'preview-note',submissionID:'token',epoch:'epoch',state:'unknown',canAcknowledge:false,terminal:null});const xc=await client(unknown);await xc.preview();await xc.preview();assert(xc.pending);assert.equal(unknown.submitted.length,1);assert.equal(unknown.acknowledgments.length,0);
const oversize=host(), oc=await client(oversize);oc.edit('🧋'.repeat(1025));await oc.preview();assert.equal(oversize.submitted.length,0);assert.match(oc.message,/4096/);
process.stdout.write('Desktop example consumer checks passed'+(nativeScript?' through actual injected Desktop script':'')+'\n');
