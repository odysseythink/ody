// Lightweight DOM contract tests; no browser installation is required.
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require('node:path').join(__dirname, '../src/app.js'), 'utf8');
class Element {
  constructor(tag) { this.tag = tag; this.children = []; this.disabled = false; }
  append(e) { this.children.push(e); }
  replaceChildren() { this.children = []; }
  setAttribute() {}
}
function setup(notification = 'not_configured', retryFails = false, settings = {}) {
  const elements = Object.fromEntries(['title','progress','message','cases','detail'].map(id => [id, new Element('div')]));
  const requests = [], events = {};
  const all = () => {
    const result = [];
    const walk = e => { result.push(e); e.children.forEach(walk); };
    Object.values(elements).forEach(walk);
    return result;
  };
  const document = {
    getElementById: id => elements[id] || all().find(e => e.id === id),
    createElement: tag => new Element(tag),
    querySelectorAll: () => all().filter(e => ['button','select','textarea'].includes(e.tag))
  };
  let state = {revision:0,submitted:settings.submitted || false,feedback:settings.feedback || [],notification,plan:{title:'<script>unsafe</script>',cases:[0,1].map(i => ({id:String(i),title:`case ${i}`,reason:'manual',prerequisites:'ready',steps:Array.from({length:settings.stepCount || 1}, () => ({action:'click',expected:'ok'}))}))}};
  const context = vm.createContext({document,location:{hash:'#secret'},window:{addEventListener:(name,fn) => events[name] = fn},confirm:() => settings.confirm !== false,
    fetch:async (url, options) => {
      requests.push({...options, url});
      if (url === '/api/notify') {
        if (retryFails) return {ok:false,text:async () => '验收结果已保存，但原会话通知未成功，请稍后重试。'};
        state.notification = 'queued';
      } else if (options.body) {
        if (settings.saveFails) return {ok:false,text:async () => 'failed/blocked steps require an explanation'};
        const update = JSON.parse(options.body); state = {...state,revision:state.revision+1,feedback:update.feedback,submitted:update.submit};
      }
      return {ok:true,json:async () => JSON.parse(JSON.stringify(state))};
    }});
  vm.runInContext(source, context);
  return {context,document,elements,requests,events};
}
const flush = () => new Promise(resolve => setImmediate(resolve));
test('renders model text safely and saves before switching cases', async () => {
  const s = setup(); await flush();
  assert.equal(s.elements.title.textContent, '<script>unsafe</script>');
  const outcome = s.document.getElementById('outcome-1');
  outcome.value = 'failed'; outcome.onchange();
  const actual = s.document.getElementById('actual-1'); actual.value = 'step 1 failed'; actual.oninput();
  await s.elements.cases.children[1].onclick();
  const saved = JSON.parse(s.requests.find(r => r.body).body);
  assert.equal(saved.feedback[0].steps[0].actual, 'step 1 failed');
  assert.equal(saved.feedback[0].outcome, 'failed');
  assert.equal(s.elements.detail.children[0].textContent, 'case 1');
  assert.equal(s.requests[0].headers.Authorization, 'Bearer secret');
});
test('warns on unsaved close and locks submitted feedback', async () => {
  const s = setup(); await flush();
  let outcome = s.document.getElementById('outcome-1'); outcome.value = 'passed'; outcome.onchange();
  let prevented = false; s.events.beforeunload({preventDefault:() => prevented = true}); assert.ok(prevented);
  await s.elements.cases.children[1].onclick();
  outcome = s.document.getElementById('outcome-1'); outcome.value = 'blocked'; outcome.onchange();
  const actual = s.document.getElementById('actual-1'); actual.value = 'device unavailable'; actual.oninput();
  const submit = s.elements.detail.children.find(e => e.textContent === '提交本轮验收');
  submit.onclick(); await flush();
  assert.equal(s.document.getElementById('outcome-1').disabled, true);
  assert.match(s.elements.message.textContent, /返回原 Ody/);
  prevented = false; s.events.beforeunload({preventDefault:() => prevented = true}); assert.equal(prevented, false);
});

async function submit(s) {
  await flush();
  for (let i = 0; i < 2; i++) {
    if (i) await s.elements.cases.children[i].onclick();
    const outcome = s.document.getElementById('outcome-1'); outcome.value = 'passed'; outcome.onchange();
  }
  s.elements.detail.children.find(e => e.textContent === '提交本轮验收').onclick();
  await flush();
}
test('queued notification tells the user no manual message is needed', async () => {
  const s = setup('queued'); await submit(s);
  assert.match(s.elements.message.textContent, /自动通知原 Ody 会话/);
  assert.match(s.elements.message.textContent, /无需再发送消息/);
  assert.ok(!s.elements.detail.children.some(e => e.textContent === '重试通知'));
});
test('pending notification can retry without resubmitting or unlocking feedback', async () => {
  const s = setup('pending'); await submit(s);
  assert.match(s.elements.message.textContent, /验收结果已保存/);
  await s.elements.detail.children.find(e => e.textContent === '重试通知').onclick();
  assert.equal(s.requests.filter(r => r.url === '/api/notify').length, 1);
  assert.match(s.elements.message.textContent, /无需再发送消息/);
  assert.equal(s.document.getElementById('actual').disabled, true);
});
test('failed retry retains saved results and retry control', async () => {
  const s = setup('pending', true); await submit(s);
  await s.elements.detail.children.find(e => e.textContent === '重试通知').onclick();
  assert.match(s.elements.message.textContent, /结果已保存/);
  assert.equal(s.document.getElementById('actual').disabled, true);
  assert.ok(s.elements.detail.children.some(e => e.textContent === '重试通知'));
});

test('records individual steps, keeps unexecuted distinct and bulk pass preserves evidence', async () => {
  const s = setup('not_configured', false, {stepCount:3}); await flush();
  const outcome = s.document.getElementById('outcome-2'); outcome.value = 'failed'; outcome.onchange();
  const actual = s.document.getElementById('actual-2'); actual.value = 'second step broken'; actual.oninput();
  const evidence = s.document.getElementById('evidence-2'); evidence.value = '/tmp/screenshot.png'; evidence.oninput();
  assert.match(s.elements.cases.children[0].textContent, /不通过/);
  assert.equal(s.document.getElementById('outcome-3').value, 'not_run');
  s.elements.detail.children.find(e => e.textContent === '本用例全部步骤通过').onclick();
  assert.equal(s.document.getElementById('outcome-3').value, 'passed');
  assert.equal(s.document.getElementById('actual-2').value, 'second step broken');
  await s.elements.cases.children[1].onclick();
  const saved = JSON.parse(s.requests.find(r => r.body).body).feedback[0];
  assert.equal(saved.outcome, 'passed');
  assert.deepEqual(saved.steps.map(s => s.step_index), [1,2,3]);
  assert.equal(saved.steps[1].evidence, '/tmp/screenshot.png');
});

test('bulk pass requires explicit confirmation', async () => {
  const s = setup('not_configured', false, {stepCount:2,confirm:false}); await flush();
  s.elements.detail.children.find(e => e.textContent === '本用例全部步骤通过').onclick();
  assert.equal(s.document.getElementById('outcome-1').value, 'not_run');
  assert.equal(s.document.getElementById('outcome-2').value, 'not_run');
});

test('legacy observations are retained without inventing step outcomes', async () => {
  const feedback = [{case_id:'0',outcome:'failed',actual:'old observation',evidence:'old link'}];
  const draft = setup('not_configured',false,{feedback}); await flush();
  assert.equal(draft.document.getElementById('outcome-1').value, 'not_run');
  assert.equal(draft.document.getElementById('actual').value, 'old observation');
  draft.elements.detail.children.find(e => e.textContent === '本用例全部步骤通过').onclick();
  await draft.elements.cases.children[1].onclick();
  const saved = JSON.parse(draft.requests.find(r => r.body).body).feedback[0];
  assert.equal(saved.actual, 'old observation'); assert.equal(saved.steps[0].outcome, 'passed');
  const history = setup('not_configured',false,{feedback,submitted:true}); await flush();
  assert.equal(history.document.getElementById('outcome-1'), undefined);
  assert.equal(history.document.getElementById('actual').disabled, true);
});

test('failed save retains step draft and prevents switching cases', async () => {
  const s = setup('not_configured',false,{saveFails:true}); await flush();
  const outcome = s.document.getElementById('outcome-1'); outcome.value = 'blocked'; outcome.onchange();
  await s.elements.cases.children[1].onclick();
  assert.equal(s.elements.detail.children[0].textContent, 'case 0');
  assert.equal(s.document.getElementById('outcome-1').value, 'blocked');
  assert.equal(s.document.getElementById('outcome-1').disabled, false);
  assert.match(s.elements.message.textContent, /未保存/);
});
