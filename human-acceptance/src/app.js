'use strict';
// Model-authored and human-authored strings are rendered as text, never HTML.
const token = location.hash.slice(1);
let run, active = 0, dirty = false, busy = false;
const $ = id => document.getElementById(id);
function node(tag, text, parent) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = text;
  if (parent) parent.append(element);
  return element;
}
function message(text) { $('message').textContent = text; }
async function api(update, path = '/api/run') {
  const response = await fetch(path, {
    method: update ? 'POST' : 'GET',
    headers: {Authorization: `Bearer ${token}`, 'Content-Type': 'application/json'},
    ...(update ? {body: JSON.stringify(update)} : {})
  });
  if (!response.ok) throw new Error(await response.text() || `HTTP ${response.status}`);
  return response.json();
}
function submittedMessage() {
  if (run.notification === 'queued') return '本轮已提交并自动通知原 Ody 会话。原会话空闲时会继续处理，无需再发送消息。';
  if (run.notification === 'pending') return '验收结果已保存，但通知原会话未成功。请点击“重试通知”；也可返回原会话告知验收已完成。';
  return '本轮已提交。当前没有自动通知连接，请返回原 Ody 会话告知验收已完成。';
}
async function retryNotification() {
  if (busy) return;
  busy = true;
  try { run = await api({}, '/api/notify'); message(submittedMessage()); render(); }
  catch (error) { message(error.message); }
  finally { busy = false; }
}
async function save(submit = false) {
  if (busy) return false;
  busy = true;
  document.querySelectorAll('button, select, textarea').forEach(input => { input.disabled = true; });
  try {
    run = await api({revision: run.revision, feedback: run.feedback, submit});
    dirty = false;
    message(submit ? submittedMessage() : '进度已保存。');
    render();
    return true;
  } catch (error) { message(`未保存：${error.message}。请保留当前内容；如有其他窗口修改，请刷新后重试。`); return false; }
  finally {
    busy = false;
    if (!run.submitted) document.querySelectorAll('button, select, textarea').forEach(input => { input.disabled = false; });
  }
}
function render() {
  $('title').textContent = run.plan.title;
  $('progress').textContent = `${run.feedback.length} / ${run.plan.cases.length} 项已反馈 · ${run.submitted ? '已提交（只读）' : '待验收'}`;
  $('cases').replaceChildren();
  run.plan.cases.forEach((c, index) => {
    const f = run.feedback.find(f => f.case_id === c.id);
    const b = node('button', `${index + 1}. ${c.title} · ${f ? ({passed:'通过',failed:'不通过',blocked:'无法执行'})[f.outcome] : '待反馈'}`, $('cases'));
    b.setAttribute('aria-current', String(index === active));
    b.onclick = async () => { if (busy) return; if (dirty && !(await save())) return; active = index; render(); };
  });
  const root = $('detail'); root.replaceChildren();
  const c = run.plan.cases[active];
  node('h2', c.title, root);
  node('h3', '为什么需要人工', root); node('p', c.reason, root);
  node('h3', '前置条件', root); node('p', c.prerequisites || '无', root);
  node('h3', '操作步骤与预期', root);
  const list = node('ol', undefined, root);
  for (const s of c.steps) node('li', `操作：${s.action}\n预期：${s.expected}`, list);
  const feedback = run.feedback.find(f => f.case_id === c.id);
  node('label', '验收结果', root).htmlFor = 'outcome';
  const outcome = node('select', undefined, root); outcome.id = 'outcome';
  for (const [value, text] of [['','请选择'],['passed','通过'],['failed','不通过'],['blocked','无法执行']]) {
    const option = node('option', text, outcome); option.value = value;
  }
  outcome.value = feedback?.outcome || '';
  node('label', '实际现象 / 出问题的步骤（不通过、无法执行时必填）', root).htmlFor = 'actual';
  const actual = node('textarea', undefined, root); actual.id = 'actual'; actual.value = feedback?.actual || ''; actual.maxLength = 2000;
  node('label', '证据说明（截图文件路径、工单链接等；首版不上传附件）', root).htmlFor = 'evidence';
  const evidence = node('textarea', undefined, root); evidence.id = 'evidence'; evidence.value = feedback?.evidence || ''; evidence.maxLength = 500;
  const change = () => {
    run.feedback = run.feedback.filter(f => f.case_id !== c.id);
    if (outcome.value) run.feedback.push({case_id: c.id, outcome: outcome.value, actual: actual.value, evidence: evidence.value});
    dirty = true; message('有未保存的修改。');
  };
  outcome.onchange = change; actual.oninput = change; evidence.oninput = change;
  for (const input of [outcome, actual, evidence]) input.disabled = run.submitted;
  if (!run.submitted) {
    const saveButton = node('button', '保存当前进度', root); saveButton.onclick = () => save();
    const submit = node('button', '提交本轮验收', root);
    submit.onclick = () => { if (confirm('提交后不能修改，本轮所有用例都已完成反馈？')) save(true); };
    node('small', '未完成或无法执行不等于通过。保存后可关闭页面，再从 Ody 重开。', root);
  } else if (run.notification === 'pending') {
    const retry = node('button', '重试通知', root); retry.onclick = retryNotification;
  }
}
window.addEventListener('beforeunload', event => { if (dirty) { event.preventDefault(); event.returnValue = ''; } });
api().then(value => { run = value; render(); if (run.submitted) message(submittedMessage()); }).catch(error => message(`无法打开验收：${error.message}。请从 Ody 重开带有访问凭证的链接。`));
