'use strict';
// Chat windows. Each window is its own conversation and lives in this page until it is closed,
// unless the user saves it: then the panel keeps it in its private folder on this computer.
// Model output is only ever shown as text, never as markup.
const chatLayer = document.createElement('div');
chatLayer.id = 'chats';
document.body.append(chatLayer);
const chats = new Map();
const efforts = ['', 'none', 'low', 'medium', 'high'];
// The panel stores up to 120 characters; the page offers fewer.
const TITLE_CHARS = 80;
let chatFront = 10;
let chatCount = 0;
let savedLoaded = false;
let reading = null;

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
  byId('chat-empty').hidden = value.chats.length > 0;
  byId('chat-open-hint').hidden = !value.chats.length;
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
  const rename = el('button', 'chat-rename', '✎'); rename.type = 'button';
  const save = el('button', 'chat-save'); save.type = 'button';
  const close = el('button', 'chat-close', '×'); close.type = 'button';
  bar.append(title, rename, save, close);
  const log = el('div', 'chat-log'); log.setAttribute('role', 'log');
  const note = el('p', 'chat-note');
  const form = el('form', 'chat-form');
  const options = el('div', 'chat-options');
  const modelName = el('span'), modelLabel = el('label'); modelLabel.append(modelName);
  const model = el('select'); modelLabel.append(model);
  const effortName = el('span'), effortLabel = el('label'); effortLabel.append(effortName);
  const effort = el('select'); effortLabel.append(effort);
  effort.replaceChildren(...efforts.map((level) => new Option('', level)));
  effort.value = chat.effort;
  options.append(modelLabel, effortLabel);
  const input = el('textarea'); input.rows = 2;
  const send = el('button', 'chat-send'); send.type = 'submit';
  const audio = chatAudio(input);
  form.append(options, input, send, audio.element);
  box.append(bar, log, note, form);
  chat.view = {
    box, log, input, model, effort, send, cancelAudio: audio.cancel,
    // The texts that change only with the language.
    relabel() {
      close.title = t('chatClose'); close.setAttribute('aria-label', t('chatClose'));
      rename.title = t('chatRenameHint'); rename.setAttribute('aria-label', t('chatRename'));
      modelName.textContent = t('chatModel'); effortName.textContent = t('chatEffort');
      for (const option of effort.options) option.textContent = t(option.value ? 'effort_' + option.value : 'effortAuto');
      input.placeholder = t('chatPlaceholder'); send.textContent = t(chat.busy ? 'chatStop' : 'chatSend');
      audio.relabel();
    },
    mark() {
      title.textContent = chat.title || t('chatUntitled'); box.setAttribute('aria-label', title.textContent);
      save.textContent = chat.saved ? '✓ ' + t('chatIsSaved') : t('chatSave'); save.setAttribute('aria-pressed', String(chat.saved));
      save.title = chat.saved ? t('chatUnsaveHint') : t('chatSaveHint');
      note.textContent = t(chat.saved ? 'chatKept' : 'chatLoose');
    }
  };
  fillModels(chat);
  model.addEventListener('focus', () => { fillModels(chat); freshModels(); });
  chat.view.relabel();
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
    if (event.button !== 0 || event.target.closest('button, input') || narrow()) return;
    const start = box.getBoundingClientRect(), dx = event.clientX - start.left, dy = event.clientY - start.top;
    bar.setPointerCapture(event.pointerId);
    const move = (next) => place(box, next.clientX - dx, next.clientY - dy);
    const end = () => { bar.removeEventListener('pointermove', move); bar.removeEventListener('pointerup', end); bar.removeEventListener('pointercancel', end); };
    bar.addEventListener('pointermove', move); bar.addEventListener('pointerup', end); bar.addEventListener('pointercancel', end);
  });
  // The title becomes a text field; Enter or leaving it keeps the new title, Escape the old one.
  const renaming = () => {
    if (bar.querySelector('.chat-title-input')) return;
    const field = el('input', 'chat-title-input'); field.type = 'text'; field.maxLength = TITLE_CHARS;
    field.value = chat.title; field.placeholder = t('chatUntitled'); field.setAttribute('aria-label', t('chatRename'));
    let done = false;
    const finish = (keep) => {
      if (done) return;
      done = true;
      const next = field.value.replace(/\s+/g, ' ').trim().slice(0, TITLE_CHARS);
      field.replaceWith(title);
      if (!keep || !next || next === chat.title) return;
      chat.title = next; chat.view.mark();
      if (chat.saved) { chat.updated = Date.now(); store(chat); }
    };
    field.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' && !event.isComposing) { event.preventDefault(); finish(true); }
      else if (event.key === 'Escape') { event.preventDefault(); finish(false); }
    });
    field.addEventListener('blur', () => finish(true));
    title.replaceWith(field);
    field.focus(); field.select();
  };
  rename.addEventListener('click', renaming);
  title.addEventListener('dblclick', renaming);
  close.addEventListener('click', () => {
    audio.cancel();
    if (reading && box.contains(reading.button)) stopReading();
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
    audio.cancel();
    if (chat.busy) { chat.busy.abort(); return; }
    const text = input.value.trim();
    if (!text) return;
    tidy(chat);
    chat.messages.push({ role: 'user', content: text });
    // Only an untitled chat is named after its first message; a title set by hand stays.
    if (!chat.title) { chat.title = text.replace(/\s+/g, ' ').slice(0, 48); chat.view.mark(); }
    input.value = ''; input.style.height = '';
    answer(chat);
  });
  return chat;
}

function chatAudio(input) {
  const Recognition = window.SpeechRecognition || window.webkitSpeechRecognition;
  const element = el('div', 'chat-audio');
  const dictate = el('button', 'link'); dictate.type = 'button';
  const mode = el('select', 'chat-audio-mode');
  const local = new Option('', 'local'), online = new Option('', 'online');
  mode.replaceChildren(local, online);
  const status = el('span', 'chat-audio-status'); status.setAttribute('role', 'status');
  const supportsLocal = Recognition && 'processLocally' in Recognition.prototype
    && typeof Recognition.available === 'function';
  element.hidden = !Recognition || !window.isSecureContext;
  element.append(mode, dictate, status);
  let current = null, pending = false, statusKey = '', attempt = 0;

  function report(key) {
    statusKey = key;
    status.textContent = key ? t(key) : '';
  }
  function relabel() {
    local.textContent = t('chatDictateLocal');
    online.textContent = t('chatDictateOnline');
    mode.setAttribute('aria-label', t('chatDictateMode'));
    dictate.textContent = t(current ? 'chatDictateStop' : 'chatDictate');
    dictate.disabled = pending || mode.value === 'local' && !supportsLocal;
    mode.disabled = pending || !!current;
    if (Recognition && mode.value === 'local' && !supportsLocal) report('chatDictateUnavailable');
    else if (mode.value === 'online' && !statusKey) report('chatDictateDisclosure');
    else report(statusKey);
  }
  function cancel() {
    attempt++;
    if (current) {
      const recognition = current;
      current = null;
      try { recognition.abort(); } catch { /* the browser already stopped it */ }
      report('');
      relabel();
    }
  }
  mode.addEventListener('change', () => {
    report(mode.value === 'online' ? 'chatDictateDisclosure' : '');
    relabel();
  });
  dictate.addEventListener('click', async () => {
    if (current) {
      try { current.stop(); } catch { cancel(); }
      return;
    }
    const run = ++attempt;
    const language = document.documentElement.lang === 'de' ? 'de-DE' : 'en-US';
    const onDevice = mode.value === 'local';
    try {
      if (onDevice) {
        pending = true; relabel();
        const options = { langs: [language], processLocally: true };
        const availability = await Recognition.available(options);
        if (!element.isConnected || run !== attempt) return;
        if (availability !== 'available') {
          if (!['downloadable', 'downloading'].includes(availability) || typeof Recognition.install !== 'function') {
            report('chatDictateUnavailable');
            return;
          }
          if (!window.confirm(t('chatDictateInstall'))) return;
          report('chatDictateInstalling');
          if (!await Recognition.install(options)) { report('chatDictateUnavailable'); return; }
        }
      } else if (!window.confirm(t('chatDictateDisclosure'))) {
        return;
      }
      if (!element.isConnected || run !== attempt) return;
      for (const chat of chats.values()) {
        if (!chat.view.box.contains(element)) chat.view.cancelAudio();
      }
      stopReading();
      const recognition = new Recognition();
      if ('processLocally' in recognition) recognition.processLocally = onDevice;
      recognition.lang = language;
      recognition.onresult = (event) => {
        if (current !== recognition) return;
        const text = Array.from(event.results).slice(event.resultIndex).filter((result) => result.isFinal)
          .map((result) => result[0].transcript.trim()).filter(Boolean).join(' ');
        if (!text) return;
        input.value += (input.value && !/\s$/.test(input.value) ? ' ' : '') + text;
        input.dispatchEvent(new Event('input'));
        input.focus();
      };
      recognition.onerror = (event) => {
        if (current === recognition && event.error !== 'aborted') report('chatDictateFailed');
      };
      recognition.onend = () => {
        if (current !== recognition) return;
        current = null;
        if (statusKey === 'chatDictateListening') report('');
        relabel();
      };
      current = recognition;
      report('chatDictateListening');
      recognition.start();
    } catch {
      current = null;
      report('chatDictateFailed');
    } finally {
      pending = false;
      relabel();
    }
  });
  return { element, relabel, cancel };
}

window.addEventListener('pagehide', () => {
  for (const chat of chats.values()) chat.view.cancelAudio();
  stopReading();
});

function stopReading() {
  if (!reading) return;
  const { button } = reading;
  reading = null;
  window.speechSynthesis.cancel();
  button.textContent = t('chatReadAloud');
}
function readAloud(message, button) {
  const wasReading = reading?.button === button;
  stopReading();
  if (wasReading) return;
  const language = document.documentElement.lang.split(/[-_]/)[0].toLowerCase();
  const voice = window.speechSynthesis.getVoices().find((voice) =>
    voice.localService && voice.lang.split(/[-_]/)[0].toLowerCase() === language);
  if (!voice) { button.textContent = t('chatNoLocalVoice'); return; }
  for (const chat of chats.values()) chat.view.cancelAudio();
  const utterance = new SpeechSynthesisUtterance(split(message).content);
  utterance.voice = voice;
  utterance.lang = voice.lang;
  reading = { utterance, button };
  button.textContent = t('chatReadStop');
  utterance.onend = utterance.onerror = () => {
    if (reading?.utterance !== utterance) return;
    reading = null;
    button.textContent = t('chatReadAloud');
  };
  try {
    window.speechSynthesis.speak(utterance);
  } catch {
    stopReading();
  }
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
// After a language switch. A window that is still answering is redrawn once its answer ends.
function relabelChats() {
  for (const chat of chats.values()) {
    chat.view.relabel(); chat.view.mark();
    if (!chat.busy) draw(chat);
  }
  if (savedLoaded) listSaved();
}
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
  if (message.role === 'assistant' && !message.stats?.live && !message.stats?.waiting && split(message).content
    && window.speechSynthesis && window.SpeechSynthesisUtterance) {
    const read = el('button', 'link', t('chatReadAloud')); read.type = 'button';
    read.addEventListener('click', () => readAloud(message, read));
    box.querySelector('.msg-meta').append(read);
  }
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
  if (meta) {
    const actions = [...meta.querySelectorAll('button')];
    meta.textContent = describe(message);
    meta.append(...actions);
  }
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
      title: chat.title, created: chat.created, updated: chat.updated,
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
