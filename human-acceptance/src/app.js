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
  const complete = run.feedback.filter(f => f.outcome !== 'not_run').length;
  $('progress').textContent = `${complete} / ${run.plan.cases.length} 项有结论 · ${run.submitted ? '已提交（只读）' : '待验收'}`;
  $('cases').replaceChildren();
  run.plan.cases.forEach((c, index) => {
    const f = run.feedback.find(f => f.case_id === c.id);
    const b = node('button', `${index + 1}. ${c.title} · ${f ? labels[f.outcome] : '待反馈'}`, $('cases'));
    b.setAttribute('aria-current', String(index === active));
    b.onclick = async () => { if (busy) return; if (dirty && !(await save())) return; active = index; render(); };
  });
  const root = $('detail'); root.replaceChildren();
  const c = run.plan.cases[active];
  node('h2', c.title, root);
  node('h3', '为什么需要人工', root); node('p', c.reason, root);
  node('h3', '前置条件', root); node('p', c.prerequisites || '无', root);
  node('h3', '操作步骤与预期', root);
  const feedback = run.feedback.find(f => f.case_id === c.id);
  const legacy = feedback && !feedback.steps;
  if (legacy) node('p', '旧版用例级反馈已保留，不能推断各步骤通过。未提交的验收请逐步骤重新确认；已提交历史保持只读。', root);
  const steps = feedback?.steps || c.steps.map((_, i) => ({step_index:i + 1,outcome:'not_run',actual:'',evidence:''}));
  const summary = node('p', `用例结果（自动汇总）：${labels[feedback?.outcome || 'not_run']}`, root);
  const changed = () => {
    run.feedback = run.feedback.filter(f => f.case_id !== c.id);
    const outcome = aggregate(steps);
    run.feedback.push({case_id:c.id,outcome,actual:actual.value,evidence:evidence.value,steps});
    summary.textContent = `用例结果（自动汇总）：${labels[outcome]}`;
    $('cases').children[active].textContent = `${active + 1}. ${c.title} · ${labels[outcome]}`;
    $('progress').textContent = `${run.feedback.filter(f => f.outcome !== 'not_run').length} / ${run.plan.cases.length} 项有结论 · 待验收`;
    dirty = true; message('有未保存的修改。');
  };
  if (!run.submitted) {
    const allPassed = node('button', '本用例全部步骤通过', root);
    allPassed.onclick = () => {
      if (busy || !confirm('确认你已实际执行并核对本用例所有步骤？这会将所有步骤标为通过，但保留现象和证据。')) return;
      steps.forEach(s => { s.outcome = 'passed'; }); changed(); render();
    };
  }
  const list = node('ol', undefined, root);
  c.steps.forEach((s, i) => {
    const item = node('li', undefined, list);
    node('p', `操作：${s.action}\n预期：${s.expected}`, item);
    if (legacy && run.submitted) { node('small', '无逐步骤记录（旧版反馈）', item); return; }
    const result = steps[i];
    node('label', '本步骤结果', item).htmlFor = `outcome-${i + 1}`;
    const outcome = node('select', undefined, item); outcome.id = `outcome-${i + 1}`;
    for (const [value, text] of Object.entries(labels)) { const option = node('option', text, outcome); option.value = value; }
    outcome.value = result.outcome;
    node('label', '本步骤实际现象（不通过、无法执行时必填）', item).htmlFor = `actual-${i + 1}`;
    const stepActual = node('textarea', undefined, item); stepActual.id = `actual-${i + 1}`; stepActual.value = result.actual; stepActual.maxLength = 2000;
    node('label', '本步骤证据说明（路径或链接，不上传附件）', item).htmlFor = `evidence-${i + 1}`;
    const stepEvidence = node('textarea', undefined, item); stepEvidence.id = `evidence-${i + 1}`; stepEvidence.value = result.evidence; stepEvidence.maxLength = 500;
    const change = () => { result.outcome = outcome.value; result.actual = stepActual.value; result.evidence = stepEvidence.value; changed(); };
    outcome.onchange = change; stepActual.oninput = change; stepEvidence.oninput = change;
    for (const input of [outcome, stepActual, stepEvidence]) input.disabled = run.submitted;
  });
  node('label', legacy ? '旧版实际现象 / 用例总体备注（保留，可选）' : '用例总体备注（可选）', root).htmlFor = 'actual';
  const actual = node('textarea', undefined, root); actual.id = 'actual'; actual.value = feedback?.actual || ''; actual.maxLength = 2000;
  node('label', '证据说明（截图文件路径、工单链接等；首版不上传附件）', root).htmlFor = 'evidence';
  const evidence = node('textarea', undefined, root); evidence.id = 'evidence'; evidence.value = feedback?.evidence || ''; evidence.maxLength = 500;
  actual.oninput = changed; evidence.oninput = changed;
  for (const input of [actual, evidence]) input.disabled = run.submitted;
  if (!run.submitted) {
    const saveButton = node('button', '保存当前进度', root); saveButton.onclick = () => save();
    const submit = node('button', '提交本轮验收', root);
    submit.onclick = () => { if (confirm('提交后不能修改，本轮所有用例都已有结论？未执行步骤不会算通过。')) save(true); };
    node('small', '未完成或无法执行不等于通过。保存后可关闭页面，再从 Ody 重开。', root);
  } else if (run.notification === 'pending') {
    const retry = node('button', '重试通知', root); retry.onclick = retryNotification;
  }
}
const labels = {not_run:'未执行',passed:'通过',failed:'不通过',blocked:'无法执行'};
function aggregate(steps) {
  if (steps.some(s => s.outcome === 'failed')) return 'failed';
  if (steps.some(s => s.outcome === 'blocked')) return 'blocked';
  return steps.some(s => s.outcome === 'not_run') ? 'not_run' : 'passed';
}
window.addEventListener('beforeunload', event => { if (dirty) { event.preventDefault(); event.returnValue = ''; } });
api().then(value => { run = value; render(); if (run.submitted) message(submittedMessage()); }).catch(error => message(`无法打开验收：${error.message}。请从 Ody 重开带有访问凭证的链接。`));
