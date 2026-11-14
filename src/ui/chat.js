'use strict';
// Conversations stay in this page. The panel writes a private copy when the user saves one.
// Model output is only ever shown as text, never as markup.
const chatLayer = byId('chats');
const chats = new Map();
const efforts = ['', 'none', 'low', 'medium', 'high'];
// The panel stores up to 120 characters; the page offers fewer.
const TITLE_CHARS = 80;
let activeChat = null;
let initialChatOpened = false;
let savedLoaded = false;
let reading = null;
const mediaNames = {
  'minimax-h3-image': 'MiniMax H3 Image',
  'minimax-h3-video': 'MiniMax H3 Turbo',
  'hyperflow-h3-flash': 'HyperFlow H3 Flash'
};

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
const modelLabel = (id) => Object.hasOwn(mediaNames, id) ? mediaNames[id] : short(id);
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
  const savedChats = value.chats.filter((saved) => !chats.has(saved.id));
  list.replaceChildren(...savedChats.map((saved) => {
    const row = el('li');
    const open = el('button', 'chat-list-title', saved.title || t('chatUntitled')); open.type = 'button';
    open.addEventListener('click', () => reopen(saved.id));
    const remove = el('button', 'chat-delete', t('chatDelete')); remove.type = 'button';
    remove.addEventListener('click', () => confirmed(remove, t('chatSure'), async () => {
      try { await chatApi('DELETE', 'chats/' + saved.id); } catch (cause) { error(cause.message); }
      const chat = chats.get(saved.id);
      if (chat) { chat.saved = false; chat.view.mark(); }
      listSaved();
    }));
    row.append(open, remove);
    return row;
  }));
  byId('chat-saved').hidden = !savedChats.length;
}
async function reopen(id) {
  if (chats.has(id)) { front(chats.get(id)); return; }
  try { openChat({ ...(await chatApi('GET', 'chats/' + id)), id, saved: true }); listSaved(); } catch (cause) { error(cause.message); }
}
byId('chat-new').addEventListener('click', () => {
  if (activeChat && !activeChat.title && !hasUnsavedWork(activeChat)) activeChat.view.input.focus();
  else openChat();
});
const historyToggle = byId('history-toggle');
const chatHistoryList = byId('chat-history');
function showHistory(show) {
  chatHistoryList.hidden = !show;
  historyToggle.setAttribute('aria-expanded', String(show));
}
historyToggle.addEventListener('click', () => showHistory(chatHistoryList.hidden));
if (matchMedia('(max-width: 700px)').matches) showHistory(false);

function startChat() {
  if (!initialChatOpened && !chats.size) openChat({}, false);
  initialChatOpened = true;
}

function renderOpenChats() {
  const items = [...chats.values()].map((chat) => {
    const row = el('li');
    const item = el('button', 'chat-list-title', chat.title || t('chatUntitled'));
    item.type = 'button';
    item.setAttribute('aria-current', String(chat === activeChat));
    item.addEventListener('click', () => front(chat));
    row.append(item);
    return row;
  });
  byId('chat-open-list').replaceChildren(...items);
  byId('chat-idle').hidden = items.length > 0;
}

/* ---- one window ---- */
// A window is a text conversation. Images and videos have their own tabs (studio.js).
function openChat(data = {}, focus = true) {
  const chat = {
    id: data.id || chatId(), title: data.title || '', created: data.created || Date.now(), updated: data.updated || 0,
    saved: !!data.saved, model: data.model || '', effort: data.effort || '',
    messages: Array.isArray(data.messages) ? data.messages.filter((message) =>
      ['user', 'assistant', 'error'].includes(message?.role) && typeof message.content === 'string') : [],
    busy: null
  };
  chats.set(chat.id, chat);
  const box = el('article', 'chat');
  const bar = el('div', 'chat-bar');
  const title = el('span', 'chat-title');
  const save = el('button', 'chat-save'); save.type = 'button';
  // Dictation and read-aloud settings live in one place per window, not on every answer.
  const speech = el('details', 'chat-menu');
  const speechLabel = el('summary');
  const speechMenu = el('div', 'chat-popover chat-speech-menu');
  speech.append(speechLabel, speechMenu);
  const conversation = el('details', 'chat-menu');
  const conversationLabel = el('summary');
  const conversationMenu = el('div', 'chat-popover chat-tool-menu');
  const rename = el('button', 'chat-tool'); rename.type = 'button';
  const close = el('button', 'chat-close'); close.type = 'button';
  conversationMenu.append(rename, close);
  conversation.append(conversationLabel, conversationMenu);
  bar.append(title, save, speech, conversation);
  const log = el('div', 'chat-log'); log.setAttribute('role', 'log');
  const thread = el('div', 'chat-thread'); log.append(thread);
  const form = el('form', 'chat-form');
  const modelName = el('span'), modelField = el('label'); modelField.append(modelName);
  const model = el('select'); modelField.append(model);
  const effortName = el('span'), effortLabel = el('label'); effortLabel.append(effortName);
  const effort = el('select'); effortLabel.append(effort);
  effort.replaceChildren(...efforts.map((level) => new Option('', level)));
  effort.value = chat.effort;
  const card = cardButton(model);
  modelField.append(card.button);
  const options = el('div', 'chat-options');
  options.append(modelField, effortLabel);
  const input = el('textarea'); input.rows = 2;
  const send = el('button', 'chat-send'); send.type = 'submit';
  const audio = chatAudio(input);
  const voice = chatVoice();
  speechMenu.append(audio.preference, voice.element);
  const sendRow = el('div', 'chat-send-row');
  sendRow.append(options, audio.element, send);
  const composer = el('div', 'chat-composer');
  composer.append(input, sendRow, audio.status);
  form.append(composer);
  box.append(bar, log, form);
  const menus = [speech, conversation];
  for (const menu of menus) {
    menu.addEventListener('toggle', () => {
      if (menu.open) for (const other of menus) if (other !== menu) other.open = false;
    });
    menu.addEventListener('keydown', (event) => {
      if (event.key === 'Escape' && menu.open) { menu.open = false; menu.querySelector('summary').focus(); }
    });
    menu.addEventListener('focusout', (event) => {
      if (menu.open && event.relatedTarget && !menu.contains(event.relatedTarget)) menu.open = false;
    });
  }
  chat.view = {
    box, log: thread, scroller: log, input, model, effort, send, voice,
    cancelAudio: audio.cancel,
    mode() {
      audio.element.hidden = !audio.select.options.length;
      speech.hidden = audio.preference.hidden && voice.element.hidden;
      send.textContent = t(chat.busy ? 'chatStop' : 'chatSend');
    },
    catalog() {
      audio.relabel(); voice.relabel(); fillModels(chat); card.update();
    },
    // The texts that change only with the language.
    relabel() {
      close.title = t('chatClose'); close.setAttribute('aria-label', t('chatClose'));
      if (!close.dataset.sure) close.textContent = t('chatClose');
      rename.title = t('chatRenameHint'); rename.setAttribute('aria-label', t('chatRename'));
      rename.textContent = t('chatRename');
      conversationLabel.textContent = t('chatActions');
      speechLabel.textContent = t('chatSpeech');
      modelName.textContent = t('chatModel'); effortName.textContent = t('chatEffort');
      input.setAttribute('aria-label', t('chatMessage'));
      input.placeholder = t('chatPlaceholder');
      for (const option of effort.options) option.textContent = t(option.value ? 'effort_' + option.value : 'effortAuto');
      this.catalog(); this.mode();
    },
    mark() {
      title.textContent = chat.title || t('chatUntitled'); box.setAttribute('aria-label', title.textContent);
      save.textContent = chat.saved ? '✓ ' + t('chatIsSaved') : t('chatSave'); save.setAttribute('aria-pressed', String(chat.saved));
      save.title = chat.saved ? t('chatUnsaveHint') : t('chatSaveHint');
      save.hidden = !chat.saved && !chat.messages.some((message) => message.role === 'user');
    }
  };
  for (const control of [model, audio.select, voice.select]) control.addEventListener('focus', freshModels);
  chat.view.relabel();
  chat.view.mark();
  draw(chat);

  chatLayer.append(box);
  front(chat, focus);
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
      chat.title = next; chat.view.mark(); renderOpenChats();
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
  rename.addEventListener('click', () => { conversation.open = false; renaming(); });
  title.addEventListener('dblclick', renaming);
  function closeChat() {
    audio.cancel();
    if (reading && box.contains(reading.button)) stopReading();
    if (chat.busy) chat.busy.abort();
    chats.delete(chat.id); box.remove();
    front([...chats.values()].at(-1) || null);
    if (chat.saved) listSaved();
    if (!activeChat) byId('chat-new').focus();
  }
  close.addEventListener('click', () => {
    if (hasUnsavedWork(chat)) confirmed(close, t('chatConfirmClose'), closeChat);
    else closeChat();
  });
  save.addEventListener('click', () => {
    if (!chat.saved) { chat.saved = true; chat.view.mark(); store(chat); return; }
    confirmed(save, t('chatUnsave'), async () => {
      try { await chatApi('DELETE', 'chats/' + chat.id); chat.saved = false; } catch (cause) { error(cause.message); }
      chat.view.mark(); listSaved();
    });
  });
  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      if (!chat.busy) form.requestSubmit();
    }
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
    nameChat(chat, text);
    input.value = ''; input.style.height = '';
    answer(chat);
  });
  return chat;
}
// A click outside an open window menu closes it.
document.addEventListener('click', (event) => {
  for (const menu of document.querySelectorAll('.chat-menu[open]')) if (!menu.contains(event.target)) menu.open = false;
});

function nameChat(chat, text) {
  if (!chat.title) {
    chat.title = text.replace(/\s+/g, ' ').slice(0, 48);
    renderOpenChats();
  }
  chat.view.mark();
}

// Dictation into `input`. `long` (the audio tab) records up to fifteen minutes and keeps
// browser recognition running until it is stopped.
function chatAudio(input, long = false) {
  const Recognition = window.SpeechRecognition || window.webkitSpeechRecognition;
  const element = el('div', 'chat-audio');
  const dictate = el('button', 'secondary'); dictate.type = 'button';
  const preference = el('label');
  const name = el('span');
  const mode = el('select', 'chat-audio-mode');
  const status = el('span', 'chat-audio-status'); status.setAttribute('role', 'status');
  const supportsLocal = Recognition && 'processLocally' in Recognition.prototype
    && typeof Recognition.available === 'function';
  const canRecord = !!(navigator.mediaDevices?.getUserMedia && window.MediaRecorder && window.AudioContext && window.OfflineAudioContext);
  preference.append(name, mode);
  element.append(dictate);
  let current = null, recording = null, stopRecording = null, pending = false, statusKey = '', attempt = 0;

  function report(key, text) {
    statusKey = key;
    status.textContent = text || (key ? t(key) : '');
    status.hidden = !key;
    status.removeAttribute('title');
  }
  function relabel() {
    const choices = Recognition ? [
      { value: 'local', label: t('chatDictateLocal') }, { value: 'online', label: t('chatDictateOnline') }
    ] : [];
    if (canRecord) for (const model of modelsFor('transcription')) {
      choices.push({ value: 'link:' + model.id, label: t(model.local ? 'chatLocalAudio' : 'chatDictateLink').replace('{model}', short(model.id)) });
    }
    const linked = choices.find((item) => item.value.startsWith('link:'))?.value;
    // The audio tab also reads files, which only a Link model does.
    const preferred = (long && linked) || (supportsLocal ? 'local' : linked || 'online');
    fillChoices(mode, window.isSecureContext ? choices : [], preferred);
    // Models arrive after the first fill: until the user picks, the preferred choice wins.
    if (!mode.dataset.chosen && [...mode.options].some((option) => option.value === preferred)) mode.value = preferred;
    preference.hidden = !mode.options.length;
    name.textContent = t('chatDictateMode');
    mode.setAttribute('aria-label', t('chatDictateMode'));
    dictate.textContent = t(current || stopRecording ? 'chatDictateStop' : 'chatDictate');
    dictate.disabled = pending && !stopRecording || mode.value === 'local' && !supportsLocal;
    mode.disabled = pending || !!current || !!recording;
    if (Recognition && mode.value === 'local' && !supportsLocal) report('chatDictateUnavailable');
    else report(statusKey === 'chatDictateUnavailable' && mode.value !== 'local' ? '' : statusKey);
  }
  function cancel() {
    attempt++;
    if (recording) {
      recording.abort(); recording = null; stopRecording = null; pending = false;
      report(''); relabel();
    }
    if (current) {
      const recognition = current;
      current = null;
      try { recognition.abort(); } catch { /* the browser already stopped it */ }
      report('');
      relabel();
    }
  }
  function appendTranscript(text) {
    if (!text) return;
    input.value += (input.value && !/\s$/.test(input.value) ? ' ' : '') + text;
    input.dispatchEvent(new Event('input'));
    input.focus();
  }
  function stopOtherAudio() {
    for (const chat of chats.values()) {
      if (!chat.view.box.contains(element)) chat.view.cancelAudio();
    }
    stopReading();
  }
  async function linkDictation(model) {
    stopOtherAudio();
    const controller = new AbortController();
    recording = controller; pending = true; relabel();
    const lang = document.documentElement.lang === 'de' ? 'de' : 'en';
    try {
      const blob = await recordMicrophone(controller.signal, (stop) => {
        stopRecording = stop; report('chatDictateListening'); relabel();
      }, long ? { seconds: MAX_AUDIO_SECONDS, bytes: 60 * MiB } : undefined);
      controller.signal.throwIfAborted();
      stopRecording = null; report('chatDictateTranscribing'); relabel();
      if (blob.size) {
        const text = await transcribeAudio(blob, model, lang, controller.signal, (piece, pieces) => {
          if (pieces > 1) report('chatDictateTranscribing', t('listenPart').replace('{i}', piece).replace('{n}', pieces));
        });
        controller.signal.throwIfAborted();
        if (!text) { report('chatDictateNoText'); return; }
        appendTranscript(text);
      } else { report('chatDictateNoText'); return; }
      if (recording === controller) report('');
    } catch (cause) {
      if (!controller.signal.aborted) {
        report('chatDictateFailed');
        if (cause.name !== 'NotAllowedError' && cause.name !== 'NotFoundError') status.title = cause.message;
      }
    } finally {
      if (recording === controller) { recording = null; stopRecording = null; pending = false; relabel(); }
    }
  }
  mode.addEventListener('change', () => {
    mode.dataset.chosen = '1';
    report('');
    relabel();
  });
  dictate.addEventListener('click', async () => {
    if (stopRecording) { stopRecording(); return; }
    if (current) {
      try { current.stop(); } catch { cancel(); }
      return;
    }
    if (mode.value.startsWith('link:')) {
      await linkDictation(mode.value.slice(5));
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
      stopOtherAudio();
      const recognition = new Recognition();
      if ('processLocally' in recognition) recognition.processLocally = onDevice;
      recognition.lang = language;
      recognition.continuous = long;
      recognition.onresult = (event) => {
        if (current !== recognition) return;
        const text = Array.from(event.results).slice(event.resultIndex).filter((result) => result.isFinal)
          .map((result) => result[0].transcript.trim()).filter(Boolean).join(' ');
        appendTranscript(text);
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
  return { element, relabel, cancel, select: mode, preference, status, busy: () => !!(current || recording) };
}

window.addEventListener('pagehide', (event) => {
  for (const chat of chats.values()) {
    chat.view.cancelAudio();
    if (chat.busy) chat.busy.abort();
  }
  stopReading();
});

function chatVoice() {
  const element = el('div', 'chat-voice');
  const modelLabel = el('label'), modelName = el('span'), select = el('select');
  modelLabel.append(modelName, select);
  const voiceLabel = el('label'), voiceName = el('span'), voice = el('input');
  voice.type = 'text'; voice.maxLength = 128; voice.autocomplete = 'off';
  const names = el('datalist'); names.id = 'voices-' + chatId();
  voice.setAttribute('list', names.id);
  voiceLabel.append(voiceName, voice, names);
  element.append(modelLabel, voiceLabel);
  const browserVoice = !!(window.speechSynthesis && window.SpeechSynthesisUtterance);
  function selectVoice() {
    const model = modelsFor('speech').find((item) => 'link:' + item.id === select.value);
    const choices = model?.voices || [];
    names.replaceChildren(...choices.map((value) => new Option(value, value)));
    voice.value = choices[0] || 'alloy';
    voiceLabel.hidden = !model;
  }
  function relabel() {
    const previous = select.value;
    const choices = browserVoice ? [{ value: 'browser', label: t('chatBrowserVoice') }] : [];
    for (const model of modelsFor('speech')) {
      const label = short(model.id);
      choices.push({ value: 'link:' + model.id, label: model.local ? t('chatLocalAudio').replace('{model}', label) : label });
    }
    fillChoices(select, choices);
    element.hidden = !choices.length;
    modelName.textContent = t('chatVoice'); voiceName.textContent = t('chatVoiceName');
    if (previous !== select.value || !voice.value) selectVoice();
  }
  select.addEventListener('change', () => { stopReading(); selectVoice(); });
  return { element, select, voice, relabel };
}

function stopReading() {
  if (!reading) return;
  const previous = reading;
  reading = null;
  previous.stop();
  previous.button.textContent = t('chatReadAloud');
}

async function readAloud(message, button, voiceSelection) {
  const wasReading = reading?.button === button;
  stopReading();
  if (wasReading) return;
  const input = split(message).content;
  const status = button.parentElement?.querySelector('.msg-audio-status');
  const report = (text) => { if (status) status.textContent = text; };
  report('');
  for (const chat of chats.values()) chat.view.cancelAudio();
  if (voiceSelection?.select.value.startsWith('link:')) {
    const controller = new AbortController(), audio = new Audio();
    let url;
    const playback = {
      button,
      stop() {
        controller.abort(); audio.pause(); audio.removeAttribute('src');
        if (url) URL.revokeObjectURL(url);
        report('');
      }
    };
    reading = playback;
    button.textContent = t('chatReadStop');
    try {
      const response = await requestSpeech({
        model: voiceSelection.select.value.slice(5), input, voice: voiceSelection.voice.value.trim(), response_format: 'wav'
      }, controller.signal, () => report(t('chatSpeechWaiting')));
      const blob = await mediaBlob(response, 32 * MiB);
      controller.signal.throwIfAborted();
      if (!/^audio\/(wav|wave|x-wav|mpeg|ogg|mp4|flac)(?:;|$)/.test(blob.type) || !blob.size) {
        throw new Error(t('chatAudioFailed'));
      }
      url = URL.createObjectURL(blob);
      report('');
      audio.onended = () => { if (reading === playback) stopReading(); };
      audio.onerror = () => {
        if (reading === playback) { stopReading(); report(t('chatAudioFailed')); }
      };
      audio.src = url;
      await audio.play();
    } catch (cause) {
      if (reading === playback) { stopReading(); report(cause.message || t('chatAudioFailed')); }
    }
    return;
  }
  if (!window.speechSynthesis || !window.SpeechSynthesisUtterance) return;
  const language = document.documentElement.lang.split(/[-_]/)[0].toLowerCase();
  const voice = window.speechSynthesis.getVoices().find((voice) =>
    voice.localService && voice.lang.split(/[-_]/)[0].toLowerCase() === language);
  if (!voice) { button.textContent = t('chatNoLocalVoice'); return; }
  const utterance = new SpeechSynthesisUtterance(input);
  utterance.voice = voice;
  utterance.lang = voice.lang;
  reading = { utterance, button, stop: () => window.speechSynthesis.cancel() };
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
function front(chat, focus = true) {
  if (activeChat && chat !== activeChat) {
    activeChat.view.cancelAudio();
    if (reading && activeChat.view.box.contains(reading.button)) stopReading();
  }
  activeChat = chat;
  for (const other of chats.values()) other.view.box.hidden = other !== chat;
  renderOpenChats();
  if (matchMedia('(max-width: 700px)').matches) showHistory(false);
  if (chat && focus) chat.view.input.focus();
}
// The connection's models, plus the one this chat last used if it is not among them.
function fillModels(chat) {
  const v = chat.view;
  const capability = new Map(modelCatalog.map((model) => [model.id, model.capability]));
  const chatModel = (id) => !capability.get(id) || capability.get(id) === 'chat';
  const list = (models || []).filter(chatModel);
  if (chat.model && chatModel(chat.model) && !list.includes(chat.model)) list.push(chat.model);
  fillChoices(v.model, list.length ? list.map((id) => ({ value: id, label: modelLabel(id) })) : [{ value: '', label: t('chatDefaultModel') }], chat.model);
  v.mode(); v.mark();
}
// A model loaded on the model computer later shows up when a model list is opened again.
let modelsAsked = 0;
async function freshModels() {
  if (Date.now() - modelsAsked < 15000) return;
  modelsAsked = Date.now();
  try {
    const result = await api('test', {});
    models = result.models; modelCatalog = result.catalog || [];
    chatModels();
  } catch { /* the ready step shows the problem */ }
}
// The model list arrives with the connection test, after a window may have opened.
// Open windows whose conversation would be lost by a restart.
function hasUnsavedWork(chat) {
  return (!chat.saved && chat.messages.length > 0) || !!chat.view.input.value.trim();
}
// Open windows and studio results that a restart would lose.
function unsavedChats() {
  return [...chats.values()].filter(hasUnsavedWork).length + (typeof studioUnsaved === 'function' ? studioUnsaved() : 0);
}
function chatModels() {
  for (const chat of chats.values()) {
    chat.view.catalog();
    if (!chat.busy && !(reading && chat.view.log.contains(reading.button))) draw(chat);
  }
  if (typeof studioModels === 'function') studioModels();
}
// After a language switch. A window that is still answering is redrawn once its answer ends.
function relabelChats() {
  renderOpenChats();
  for (const chat of chats.values()) {
    chat.view.relabel(); chat.view.mark();
    if (!chat.busy) draw(chat);
  }
  if (savedLoaded) listSaved();
  if (typeof studioRelabel === 'function') studioRelabel();
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
  if (message.role === 'assistant') {
    box.append(el('div', 'msg-progress'));
    if (!message.stats?.live && !message.stats?.waiting && describe(message)) {
      const details = el('details', 'msg-details');
      details.append(el('summary', '', t('chatDetails')), el('div', 'msg-meta', describe(message)));
      box.append(details);
    }
  }
  paint(box, message);
  const actions = el('div', 'msg-actions');
  if (message.role === 'assistant' && !message.stats?.live && !message.stats?.waiting && split(message).content
    && chat.view.voice.select.value) {
    const read = el('button', 'link', t('chatReadAloud')); read.type = 'button';
    const status = el('span', 'msg-audio-status'); status.setAttribute('role', 'status');
    read.addEventListener('click', () => readAloud(message, read, chat.view.voice));
    actions.append(read, status);
  }
  if (last && !chat.busy && message.role !== 'user') {
    const retry = el('button', 'link', t('chatRetry')); retry.type = 'button';
    retry.addEventListener('click', () => {
      if (chat.busy) return;
      const lastUser = chat.messages.findLastIndex((entry) => entry.role === 'user');
      if (lastUser < 0) return;
      chat.messages = chat.messages.filter((entry, index) => index <= lastUser);
      answer(chat, message.model || chat.model, message.effort ?? chat.effort);
    });
    actions.append(retry);
  }
  if (actions.childElementCount) box.append(actions);
  return box;
}
function paint(box, message) {
  const shown = split(message);
  writeText(box.querySelector('.msg-text'), shown.content);
  const think = box.querySelector('.msg-think');
  if (think) { think.hidden = !shown.reasoning; think.querySelector('.msg-think-text').textContent = shown.reasoning; }
  const progress = box.querySelector('.msg-progress');
  if (progress) progress.textContent = message.stats?.waiting ? t('chatWaiting') : message.stats?.live ? t('chatWriting') : '';
}
// Some servers send the thinking inside the answer, between <think> tags.
function split(message) {
  const content = message.content || '', reasoning = message.reasoning || '';
  if (!content.trimStart().startsWith('<think>')) return { content, reasoning };
  const rest = content.trimStart().slice(7), end = rest.indexOf('</think>');
  return end < 0 ? { content: '', reasoning: reasoning + rest } : { content: rest.slice(end + 8).trimStart(), reasoning: reasoning + rest.slice(0, end) };
}
function draw(chat) {
  const { log, scroller } = chat.view;
  const follow = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 40;
  if (reading && log.contains(reading.button)) stopReading();
  log.replaceChildren(...chat.messages.map((message, index) => view(chat, message, index === chat.messages.length - 1)));
  if (follow || !chat.busy) scroller.scrollTop = scroller.scrollHeight;
}
async function store(chat) {
  chat.updated = chat.updated || Date.now();
  try {
    await chatApi('PUT', 'chats/' + chat.id, {
      title: chat.title, created: chat.created, updated: chat.updated,
      model: chat.model, effort: chat.effort, messages: chat.messages.filter((m) => m.role === 'user' || m.role === 'assistant')
    });
  } catch (cause) { chat.view.mark(); error(cause.message); return; }
  listSaved();
}

/* ---- one answer ---- */
async function answer(chat, model = chat.view.model.value, effort = chat.view.effort.value) {
  const v = chat.view;
  const message = { role: 'assistant', content: '', reasoning: '', model, effort, stats: { live: true } };
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
  chat.busy = controller; v.mode(); v.send.classList.add('stop');
  draw(chat);
  const box = v.log.lastElementChild;
  let started = performance.now();
  let first = 0, pieces = 0, usage = null, timings = null, failed = '', queued = false, finish = '';
  const live = () => {
    queued = false;
    if (first && pieces > 1) { message.stats.rate = (pieces - 1) / ((performance.now() - first) / 1000); message.stats.estimated = true; }
    message.stats.tokens = pieces;
    const follow = v.scroller.scrollHeight - v.scroller.scrollTop - v.scroller.clientHeight < 40;
    paint(box, message);
    if (follow) v.scroller.scrollTop = v.scroller.scrollHeight;
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
  chat.busy = null; v.mode(); v.send.classList.remove('stop');
  chat.updated = Date.now();
  draw(chat);
  if (chat.saved) store(chat);
}
