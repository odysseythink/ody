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
function setup(notification = 'not_configured', retryFails = false) {
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
  let state = {revision:0,submitted:false,feedback:[],notification,plan:{title:'<script>unsafe</script>',cases:[0,1].map(i => ({id:String(i),title:`case ${i}`,reason:'manual',prerequisites:'ready',steps:[{action:'click',expected:'ok'}]}))}};
  const context = vm.createContext({document,location:{hash:'#secret'},window:{addEventListener:(name,fn) => events[name] = fn},confirm:() => true,
    fetch:async (url, options) => {
      requests.push({...options, url});
      if (url === '/api/notify') {
        if (retryFails) return {ok:false,text:async () => '验收结果已保存，但原会话通知未成功，请稍后重试。'};
        state.notification = 'queued';
      } else if (options.body) { const update = JSON.parse(options.body); state = {...state,revision:state.revision+1,feedback:update.feedback,submitted:update.submit}; }
      return {ok:true,json:async () => JSON.parse(JSON.stringify(state))};
    }});
  vm.runInContext(source, context);
  return {context,document,elements,requests,events};
}
const flush = () => new Promise(resolve => setImmediate(resolve));
test('renders model text safely and saves before switching cases', async () => {
  const s = setup(); await flush();
  assert.equal(s.elements.title.textContent, '<script>unsafe</script>');
  const outcome = s.document.getElementById('outcome');
  outcome.value = 'failed'; outcome.onchange();
  const actual = s.document.getElementById('actual'); actual.value = 'step 1 failed'; actual.oninput();
  await s.elements.cases.children[1].onclick();
  const saved = JSON.parse(s.requests.find(r => r.body).body);
  assert.equal(saved.feedback[0].actual, 'step 1 failed');
  assert.equal(saved.feedback[0].outcome, 'failed');
  assert.equal(s.elements.detail.children[0].textContent, 'case 1');
  assert.equal(s.requests[0].headers.Authorization, 'Bearer secret');
});
test('warns on unsaved close and locks submitted feedback', async () => {
  const s = setup(); await flush();
  let outcome = s.document.getElementById('outcome'); outcome.value = 'passed'; outcome.onchange();
  let prevented = false; s.events.beforeunload({preventDefault:() => prevented = true}); assert.ok(prevented);
  await s.elements.cases.children[1].onclick();
  outcome = s.document.getElementById('outcome'); outcome.value = 'blocked'; outcome.onchange();
  const actual = s.document.getElementById('actual'); actual.value = 'device unavailable'; actual.oninput();
  const submit = s.elements.detail.children.find(e => e.textContent === '提交本轮验收');
  submit.onclick(); await flush();
  assert.equal(s.document.getElementById('outcome').disabled, true);
  assert.match(s.elements.message.textContent, /返回原 Ody/);
  prevented = false; s.events.beforeunload({preventDefault:() => prevented = true}); assert.equal(prevented, false);
});

async function submit(s) {
  await flush();
  for (let i = 0; i < 2; i++) {
    if (i) await s.elements.cases.children[i].onclick();
    const outcome = s.document.getElementById('outcome'); outcome.value = 'passed'; outcome.onchange();
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
