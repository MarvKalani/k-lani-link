'use strict';
// Chat windows. Each window is its own conversation and lives in this page until it is closed,
// unless the user saves it: then the panel keeps it in its private folder on this computer.
// Model output is only ever shown as text, never as markup.
const chatLayer = document.createElement('div');
chatLayer.id = 'chats';
document.body.append(chatLayer);
const chats = new Map();
const efforts = ['', 'none', 'low', 'medium', 'high'];
let chatFront = 10;
let chatCount = 0;
let savedLoaded = false;

function el(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}
function chatId() {
  const bytes = new Uint8Array(16); crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}
function said(value) {
  const message = typeof value.error === 'string' ? value.error : value.error && value.error.message;
  return (language === 'de' && serverDe[message]) || message || t('unexpected');
}
async function chatApi(method, path, data) {
  const response = await fetch('/api/' + path, {
    method, cache: 'no-store',
    headers: { Authorization: 'Bearer ' + token, ...(data === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: data === undefined ? undefined : JSON.stringify(data)
  });
  const value = await response.json().catch(() => ({ error: t('unexpected') }));
  if (!response.ok) throw new Error(said(value));
  return value;
}
// A second click within three seconds confirms; nothing opens a browser dialog.
function confirmed(button, question, work) {
  if (button.dataset.sure) { delete button.dataset.sure; work(); return; }
  const label = button.textContent; button.dataset.sure = '1'; button.textContent = question;
  setTimeout(() => { if (button.dataset.sure) { delete button.dataset.sure; button.textContent = label; } }, 3000);
}
// "/models/Name-Q8_0.gguf" → "Name-Q8_0"
const short = (model) => model.replace(/^.*[\\/]/, '').replace(/\.gguf(@\d+)?$/i, '$1');
const thousands = (n) => n >= 10000 ? Math.round(n / 1000) + 'k' : n >= 1000 ? (n / 1000).toFixed(1) + 'k' : String(n);

/* ---- saved chats in the panel ---- */
async function chatsReady() {
  if (savedLoaded) return;
  savedLoaded = true;
  await listSaved();
}
async function listSaved() {
  let value;
  try { value = await chatApi('GET', 'chats'); } catch { savedLoaded = false; return; }
  const list = byId('chat-list');
  list.replaceChildren(...value.chats.map((saved) => {
    const row = el('li');
    const open = el('button', 'link', saved.title || t('chatUntitled')); open.type = 'button';
    open.addEventListener('click', () => reopen(saved.id));
    const when = el('small', '', (saved.updated ? new Date(saved.updated).toLocaleString(language, { dateStyle: 'short', timeStyle: 'short' }) + ' · ' : '') + saved.messages + ' ' + t('chatMessages'));
    const remove = el('button', 'secondary small', t('chatDelete')); remove.type = 'button';
    remove.addEventListener('click', () => confirmed(remove, t('chatSure'), async () => {
      try { await chatApi('DELETE', 'chats/' + saved.id); } catch (cause) { error(cause.message); }
      const chat = chats.get(saved.id);
      if (chat) { chat.saved = false; chat.view.mark(); }
      listSaved();
    }));
    row.append(open, when, remove);
    return row;
  }));
  byId('chat-where').textContent = t('chatWhere') + ' ' + value.directory;
  byId('chat-saved').hidden = !value.chats.length;
}
async function reopen(id) {
  if (chats.has(id)) { front(chats.get(id)); return; }
  try { openChat({ ...(await chatApi('GET', 'chats/' + id)), id, saved: true }); } catch (cause) { error(cause.message); }
}
byId('chat-new').addEventListener('click', () => openChat());

/* ---- one window ---- */
function openChat(data = {}) {
  const chat = {
    id: data.id || chatId(), title: data.title || '', created: data.created || Date.now(), updated: data.updated || 0,
    saved: !!data.saved, model: data.model || '', effort: data.effort || '',
    messages: Array.isArray(data.messages) ? data.messages : [], busy: null
  };
  chats.set(chat.id, chat);
  const box = el('div', 'chat'); box.setAttribute('role', 'dialog');
  const bar = el('div', 'chat-bar');
  const title = el('span', 'chat-title');
  const save = el('button', 'chat-save'); save.type = 'button';
  const close = el('button', 'chat-close', '×'); close.type = 'button'; close.title = t('chatClose'); close.setAttribute('aria-label', t('chatClose'));
  bar.append(title, save, close);
  const log = el('div', 'chat-log'); log.setAttribute('role', 'log');
  const note = el('p', 'chat-note');
  const form = el('form', 'chat-form');
  const options = el('div', 'chat-options');
  const modelLabel = el('label'); modelLabel.append(el('span', '', t('chatModel')));
  const model = el('select'); modelLabel.append(model);
  const effortLabel = el('label'); effortLabel.append(el('span', '', t('chatEffort')));
  const effort = el('select'); effortLabel.append(effort);
  effort.replaceChildren(...efforts.map((level) => new Option(t(level ? 'effort_' + level : 'effortAuto'), level)));
  effort.value = chat.effort;
  options.append(modelLabel, effortLabel);
  const input = el('textarea'); input.rows = 2; input.placeholder = t('chatPlaceholder');
  const send = el('button', 'chat-send', t('chatSend')); send.type = 'submit';
  form.append(options, input, send);
  box.append(bar, log, note, form);
  chat.view = {
    box, log, input, model, effort, send,
    mark() {
      title.textContent = chat.title || t('chatUntitled'); box.setAttribute('aria-label', title.textContent);
      save.textContent = chat.saved ? '✓ ' + t('chatIsSaved') : t('chatSave'); save.setAttribute('aria-pressed', String(chat.saved));
      save.title = chat.saved ? t('chatUnsaveHint') : t('chatSaveHint');
      note.textContent = t(chat.saved ? 'chatKept' : 'chatLoose');
    }
  };
  fillModels(chat);
  model.addEventListener('focus', () => { fillModels(chat); freshModels(); });
  chat.view.mark();
  draw(chat);

  const narrow = () => matchMedia('(max-width: 640px)').matches;
  chatLayer.append(box);
  const step = (chatCount++ % 6) * 28;
  place(box, innerWidth - box.offsetWidth - 24 - step, 24 + step);
  front(chat);
  input.focus();

  box.addEventListener('pointerdown', () => front(chat));
  bar.addEventListener('pointerdown', (event) => {
    if (event.button !== 0 || event.target.closest('button') || narrow()) return;
    const start = box.getBoundingClientRect(), dx = event.clientX - start.left, dy = event.clientY - start.top;
    bar.setPointerCapture(event.pointerId);
    const move = (next) => place(box, next.clientX - dx, next.clientY - dy);
    const end = () => { bar.removeEventListener('pointermove', move); bar.removeEventListener('pointerup', end); bar.removeEventListener('pointercancel', end); };
    bar.addEventListener('pointermove', move); bar.addEventListener('pointerup', end); bar.addEventListener('pointercancel', end);
  });
  close.addEventListener('click', () => {
    if (chat.busy) chat.busy.abort();
    chats.delete(chat.id); box.remove();
  });
  save.addEventListener('click', () => {
    if (!chat.saved) { chat.saved = true; chat.view.mark(); store(chat); return; }
    confirmed(save, t('chatUnsave'), async () => {
      try { await chatApi('DELETE', 'chats/' + chat.id); chat.saved = false; } catch (cause) { note.textContent = cause.message; }
      chat.view.mark(); listSaved();
    });
  });
  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); form.requestSubmit(); }
  });
  input.addEventListener('input', () => { input.style.height = 'auto'; input.style.height = Math.min(input.scrollHeight + 4, 180) + 'px'; });
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    if (chat.busy) { chat.busy.abort(); return; }
    const text = input.value.trim();
    if (!text) return;
    tidy(chat);
    chat.messages.push({ role: 'user', content: text });
    if (!chat.title) { chat.title = text.replace(/\s+/g, ' ').slice(0, 48); chat.view.mark(); }
    input.value = ''; input.style.height = '';
    answer(chat);
  });
  return chat;
}
function place(box, x, y) {
  box.style.left = Math.min(Math.max(x, 60 - box.offsetWidth), innerWidth - 60) + 'px';
  box.style.top = Math.min(Math.max(y, 0), innerHeight - 40) + 'px';
}
function front(chat) {
  for (const other of chats.values()) other.view.box.classList.toggle('front', other === chat);
  chat.view.box.style.zIndex = String(++chatFront);
}
// The connection's models, plus the one this chat last used if it is not among them.
function fillModels(chat) {
  const select = chat.view.model, chosen = select.value || chat.model;
  const list = [...(models || [])];
  if (chosen && !list.includes(chosen)) list.push(chosen);
  select.replaceChildren(...(list.length ? list.map((id) => new Option(short(id), id)) : [new Option(t('chatDefaultModel'), '')]));
  select.value = chosen && list.includes(chosen) ? chosen : list[0] || '';
}
// A model loaded on the model computer later shows up when a model list is opened again.
let modelsAsked = 0;
async function freshModels() {
  if (Date.now() - modelsAsked < 15000) return;
  modelsAsked = Date.now();
  try { const list = (await api('test', {})).models; if (list.length) { models = list; chatModels(); } } catch { /* the ready step shows the problem */ }
}
// The model list arrives with the connection test, after a window may have opened.
// Open windows whose conversation would be lost by a restart.
function unsavedChats() { return [...chats.values()].filter((chat) => !chat.saved && chat.messages.length).length; }
function chatModels() { for (const chat of chats.values()) fillModels(chat); }
// Errors and empty answers are not part of the conversation the model sees.
function tidy(chat) {
  chat.messages = chat.messages.filter((m) => m.role !== 'error' && !(m.role === 'assistant' && !m.content && !m.reasoning));
}

/* ---- messages ---- */
function writeText(target, value) {
  // ``` fences become code blocks; everything stays text.
  target.replaceChildren(...value.split('```').map((part, index) => {
    if (index % 2 === 0) return document.createTextNode(part);
    const block = el('pre'), line = part.indexOf('\n');
    block.textContent = line >= 0 ? part.slice(line + 1) : part;
    return block;
  }));
}
function describe(message) {
  const s = message.stats || {}, bits = [];
  if (message.model) bits.push(short(message.model));
  if (message.effort) bits.push(t('chatEffort') + ': ' + t('effort_' + message.effort));
  if (s.rate) bits.push((s.estimated ? '≈ ' : '') + s.rate.toFixed(1) + ' tok/s');
  if (s.tokens) bits.push(s.tokens + ' tok');
  if (s.first != null) bits.push(t('chatFirst') + ' ' + (s.first / 1000).toFixed(1) + ' s');
  if (s.read) bits.push(t('chatRead') + ' ' + Math.round(s.read) + ' tok/s');
  if (s.cached) bits.push(t('chatCache') + ' ' + s.cached + '%');
  if (s.context) bits.push(t('chatContext') + ' ' + thousands(s.context));
  if (s.stopped) bits.push(t('chatStopped'));
  if (s.length) bits.push(t('chatLength'));
  if (s.waiting) bits.push(t('chatWaiting'));
  else if (s.live) bits.push(t('chatWriting'));
  return bits.join(' · ');
}
function view(chat, message, last) {
  const box = el('div', 'msg ' + message.role);
  if (message.role === 'assistant') {
    const think = el('details', 'msg-think'); think.append(el('summary', '', t('chatThinking')), el('div', 'msg-think-text'));
    box.append(think);
  }
  box.append(el('div', 'msg-text'));
  if (message.role !== 'user') box.append(el('div', 'msg-meta'));
  paint(box, message);
  if (last && !chat.busy && message.role !== 'user') {
    const retry = el('button', 'link', '↻ ' + t('chatRetry')); retry.type = 'button';
    retry.addEventListener('click', () => {
      if (chat.busy) return;
      while (chat.messages.length && chat.messages[chat.messages.length - 1].role !== 'user') chat.messages.pop();
      if (chat.messages.length) answer(chat);
    });
    box.querySelector('.msg-meta').append(retry);
  }
  return box;
}
function paint(box, message) {
  const shown = split(message);
  writeText(box.querySelector('.msg-text'), shown.content);
  const think = box.querySelector('.msg-think');
  if (think) { think.hidden = !shown.reasoning; think.querySelector('.msg-think-text').textContent = shown.reasoning; }
  const meta = box.querySelector('.msg-meta');
  if (meta) { const retry = meta.querySelector('button'); meta.textContent = describe(message); if (retry) meta.append(retry); }
}
// Some servers send the thinking inside the answer, between <think> tags.
function split(message) {
  const content = message.content || '', reasoning = message.reasoning || '';
  if (!content.trimStart().startsWith('<think>')) return { content, reasoning };
  const rest = content.trimStart().slice(7), end = rest.indexOf('</think>');
  return end < 0 ? { content: '', reasoning: reasoning + rest } : { content: rest.slice(end + 8).trimStart(), reasoning: reasoning + rest.slice(0, end) };
}
function draw(chat) {
  const log = chat.view.log, follow = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  log.replaceChildren(...chat.messages.map((message, index) => view(chat, message, index === chat.messages.length - 1)));
  if (follow || !chat.busy) log.scrollTop = log.scrollHeight;
}
async function store(chat) {
  chat.updated = chat.updated || Date.now();
  try {
    await chatApi('PUT', 'chats/' + chat.id, {
      title: chat.title || t('chatUntitled'), created: chat.created, updated: chat.updated,
      model: chat.model, effort: chat.effort, messages: chat.messages.filter((m) => m.role !== 'error')
    });
  } catch (cause) { chat.view.mark(); chat.view.box.querySelector('.chat-note').textContent = cause.message; return; }
  listSaved();
}

/* ---- one answer ---- */
async function answer(chat) {
  const v = chat.view;
  const message = { role: 'assistant', content: '', reasoning: '', model: v.model.value, effort: v.effort.value, stats: { live: true } };
  chat.model = message.model; chat.effort = message.effort;
  const history = chat.messages.filter((m) => (m.role === 'user' || m.role === 'assistant') && m.content).map((m) => ({ role: m.role, content: split(m).content || m.content }));
  const request = { messages: history };
  if (message.model) request.model = message.model;
  if (message.effort) {
    request.reasoning_effort = message.effort;
    request.chat_template_kwargs = message.effort === 'none' ? { enable_thinking: false } : { reasoning_effort: message.effort };
  }
  chat.messages.push(message);
  const controller = new AbortController();
  chat.busy = controller; v.send.textContent = t('chatStop'); v.send.classList.add('stop');
  draw(chat);
  const box = v.log.lastElementChild;
  let started = performance.now();
  let first = 0, pieces = 0, usage = null, timings = null, failed = '', queued = false, finish = '';
  const live = () => {
    queued = false;
    if (first && pieces > 1) { message.stats.rate = (pieces - 1) / ((performance.now() - first) / 1000); message.stats.estimated = true; }
    message.stats.tokens = pieces;
    const follow = v.log.scrollHeight - v.log.scrollTop - v.log.clientHeight < 40;
    paint(box, message);
    if (follow) v.log.scrollTop = v.log.scrollHeight;
  };
  try {
    let response;
    // The model computer takes a limited number of requests at once: this window waits its turn.
    for (;;) {
      started = performance.now();
      response = await fetch('/api/chat', {
        method: 'POST', cache: 'no-store', signal: controller.signal,
        headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' }, body: JSON.stringify(request)
      });
      if (response.status !== 429) break;
      message.stats.waiting = true; paint(box, message);
      await new Promise((resolve) => setTimeout(resolve, 1500));
      if (controller.signal.aborted) throw new DOMException('stopped', 'AbortError');
    }
    message.stats.waiting = false;
    if (!response.ok) throw new Error(said(await response.json().catch(() => ({}))));
    const reader = response.body.getReader(), decoder = new TextDecoder();
    let buffer = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let end;
      while ((end = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, end).trim(); buffer = buffer.slice(end + 1);
        if (!line.startsWith('data:')) continue;
        const data = line.slice(5).trim();
        if (data === '[DONE]') continue;
        let chunk;
        try { chunk = JSON.parse(data); } catch { continue; }
        if (chunk.error) throw new Error(said(chunk));
        const choice = (chunk.choices && chunk.choices[0]) || {}, delta = choice.delta || {};
        if (choice.finish_reason) finish = choice.finish_reason;
        const thought = delta.reasoning_content || delta.reasoning || '', text = delta.content || '';
        if (thought || text) { if (!first) first = performance.now(); pieces++; }
        message.reasoning += thought; message.content += text;
        if (chunk.usage) usage = chunk.usage;
        if (chunk.timings) timings = chunk.timings;
        if (!queued) { queued = true; requestAnimationFrame(live); }
      }
    }
  } catch (cause) {
    if (cause.name === 'AbortError') message.stats.stopped = true;
    else failed = cause instanceof TypeError ? t('chatCut') : cause.message;
  }
  const finished = performance.now();
  const shown = split(message);
  message.content = shown.content; message.reasoning = shown.reasoning;
  const s = { tokens: (usage && usage.completion_tokens) || (timings && timings.predicted_n) || pieces };
  if (timings && timings.predicted_per_second) s.rate = timings.predicted_per_second;
  else if (first && pieces > 1) { s.rate = (pieces - 1) / ((finished - first) / 1000); s.estimated = true; }
  if (first) s.first = first - started;
  // Like Basalt's speed card: how fast the prompt was read, and how much of it came from the cache.
  if (timings && timings.prompt_n > 0 && timings.prompt_per_second) s.read = timings.prompt_per_second;
  if (timings && timings.cache_n > 0) s.cached = Math.round(timings.cache_n / (timings.cache_n + (timings.prompt_n || 0)) * 100);
  if (usage && usage.total_tokens) s.context = usage.total_tokens;
  else if (timings && timings.prompt_n != null) s.context = (timings.cache_n || 0) + timings.prompt_n + (timings.predicted_n || 0);
  if (message.stats.stopped) s.stopped = true;
  if (finish === 'length') s.length = true;
  message.stats = s;
  if (!message.content && !message.reasoning) chat.messages.pop();
  if (failed) chat.messages.push({ role: 'error', content: failed });
  chat.busy = null; v.send.textContent = t('chatSend'); v.send.classList.remove('stop');
  chat.updated = Date.now();
  draw(chat);
  if (chat.saved) store(chat);
}
