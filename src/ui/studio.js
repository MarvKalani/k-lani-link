'use strict';
// Images and videos, each in its own tab with its own form and result list. Results stay in
// this page only, like an unsaved chat; the prompt and the model output are shown as text.
const studios = {};

function studioView(kind) {
  const part = (name) => byId(kind + '-' + name);
  const studio = {
    kind, results: [], busy: null, reference: null, referenceUrl: null,
    form: part('form'), prompt: part('prompt'), model: part('model'), go: part('go'),
    problem: part('problem'), list: part('results'), empty: part('empty')
  };
  studios[kind] = studio;
  studio.card = cardButton(studio.model);
  studio.model.parentElement.append(studio.card.button);
  if (kind === 'image') studio.inputs = [];
  studio.model.addEventListener('focus', freshModels);
  // Another model may take input images, so the results' actions change with it.
  studio.model.addEventListener('change', () => {
    report(studio, ''); studioHint(studio);
    if (!studio.busy) studioDraw(studio);
  });
  studio.prompt.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      if (!studio.busy) studio.form.requestSubmit();
    }
  });
  studio.form.addEventListener('submit', (event) => {
    event.preventDefault();
    if (studio.busy) { studio.busy.abort(); return; }
    generate(studio);
  });
  return studio;
}

function report(studio, key) {
  studio.problem.dataset.key = key;
  studio.problem.textContent = key ? t(key) : '';
  studio.problem.hidden = !key;
}

function studioChoice(studio) {
  return modelsFor(studio.kind).find((model) => model.id === studio.model.value);
}

// What the selected model takes: a video model's start image, an image model's input images.
function studioHint(studio) {
  if (studio.kind === 'image') { inputsHint(); return; }
  byId('video-start-hint').textContent = t(studioChoice(studio)?.requires_input_reference ? 'studioStartRequired' : 'studioStartOptional');
}

/* Input images: an image model that edits takes one, one that combines takes several. */
const IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/webp'];
const INPUT_BYTES = 7.5 * MiB;
function inputLimit() {
  return studioChoice(studios.image)?.max_input_images || 0;
}
function inputsHint() {
  const studio = studios.image, max = inputLimit(), count = studio.inputs.length;
  byId('image-inputs').hidden = !max;
  studio.form.querySelector('h2').textContent = t(!max || !count ? 'studioImageTitle' : count > 1 ? 'imageCombineTitle' : 'imageEditTitle');
  if (!max) return;
  byId('image-inputs-label').textContent = t(max > 1 ? 'imageInputsMany' : 'imageInputsOne').replace('{n}', max);
  byId('image-inputs-hint').textContent = t(studioChoice(studio)?.requires_input_image ? 'imageInputsRequired'
    : max > 1 ? 'imageInputsCombine' : 'imageInputsEdit').replace('{n}', max);
  byId('image-input-add').hidden = count >= max;
  byId('image-input-add').disabled = !!studio.busy;
}
function drawInputs() {
  const studio = studios.image;
  byId('image-input-list').replaceChildren(...studio.inputs.map((input) => {
    const item = el('li', 'image-input');
    const preview = el('img'); preview.src = input.url; preview.alt = input.file.name;
    const remove = el('button', 'link', t('studioRemove')); remove.type = 'button';
    remove.disabled = !!studio.busy;
    remove.addEventListener('click', () => {
      URL.revokeObjectURL(input.url);
      studio.inputs = studio.inputs.filter((entry) => entry !== input);
      drawInputs();
    });
    item.append(preview, el('span', 'image-input-name', input.file.name), remove);
    return item;
  }));
  inputsHint();
}
function addInputs(files) {
  const studio = studios.image;
  for (const file of files) {
    if (!IMAGE_TYPES.includes(file.type)) { report(studio, 'imageInputType'); continue; }
    if (studio.inputs.length >= inputLimit()) { report(studio, 'imageInputsFull'); break; }
    studio.inputs.push({ file, url: URL.createObjectURL(file) });
  }
  drawInputs();
}

function setReference(file) {
  const studio = studios.video;
  if (studio.referenceUrl) URL.revokeObjectURL(studio.referenceUrl);
  studio.reference = file;
  const previewable = file && file.size <= 7 * MiB && ['image/png', 'image/jpeg'].includes(file.type);
  studio.referenceUrl = previewable ? URL.createObjectURL(file) : null;
  const preview = byId('video-start-preview');
  if (studio.referenceUrl) preview.src = studio.referenceUrl;
  else preview.removeAttribute('src');
  preview.hidden = !studio.referenceUrl;
  byId('video-start-name').textContent = file?.name || '';
  byId('video-start-remove').hidden = !file;
  byId('video-start').classList.toggle('chosen', !!file);
  report(studio, '');
}

async function generate(studio) {
  const kind = studio.kind, model = studio.model.value, prompt = studio.prompt.value.trim();
  if (!prompt) { studio.prompt.focus(); return; }
  if (!model) return;
  const inputs = kind === 'image' && inputLimit() ? studio.inputs.map((input) => input.file) : [];
  if (kind === 'image') {
    if (studioChoice(studio)?.requires_input_image && !inputs.length) { report(studio, 'imageInputsRequired'); return; }
    if (inputs.length > inputLimit()) { report(studio, 'imageInputsFull'); return; }
    if (inputs.reduce((sum, file) => sum + file.size, 0) > INPUT_BYTES) { report(studio, 'imageInputsTooLarge'); return; }
  }
  const reference = kind === 'video' ? studio.reference : null;
  if (kind === 'video') {
    if (studioChoice(studio)?.requires_input_reference && !reference) { report(studio, 'chatImageRequired'); return; }
    if (reference?.size > 7 * MiB) { report(studio, 'chatImageTooLarge'); return; }
    if (reference && !['image/png', 'image/jpeg'].includes(reference.type)) { report(studio, 'chatVideoReference'); return; }
  }
  report(studio, '');
  const result = { kind, model, prompt, status: 'chatMediaWaiting', inputs: inputs.length };
  studio.results.unshift(result);
  const controller = new AbortController();
  studio.busy = controller;
  studioDraw(studio);
  try {
    result.blob = await generateMedia(kind, model, prompt, reference, controller.signal, (status) => {
      if (result.status !== status) { result.status = status; studioDraw(studio); }
    }, inputs);
    controller.signal.throwIfAborted();
    const ext = result.blob.type.split('/')[1];
    result.name = `${model.replace(/[^\w.-]/g, '-').slice(0, 100) || kind}.${ext === 'jpeg' ? 'jpg' : ext}`;
    result.url = URL.createObjectURL(result.blob);
  } catch (cause) {
    result.error = cause.name === 'AbortError' ? t('chatStopped') : cause.message || t('chatMediaFailed');
  } finally {
    delete result.status;
    studio.busy = null;
    studioDraw(studio);
  }
}

function dropResult(studio, result) {
  if (result.url) URL.revokeObjectURL(result.url);
  studio.results = studio.results.filter((entry) => entry !== result);
  studioDraw(studio);
}

function studioCard(studio, result) {
  const card = el('li', 'studio-card');
  if (result.url) {
    // Kept across redraws, so a playing video does not start over.
    if (!result.preview) {
      const preview = el(result.kind === 'video' ? 'video' : 'img', 'studio-media');
      if (result.kind === 'video') { preview.controls = true; preview.playsInline = true; preview.preload = 'metadata'; }
      else preview.alt = result.prompt;
      preview.src = result.url;
      result.preview = preview;
    }
    card.append(result.preview);
  } else if (result.status) {
    const working = el('div', 'studio-working');
    working.append(el('span', 'studio-spinner'), el('span', '', t(result.status)));
    working.setAttribute('role', 'status');
    card.append(working);
  } else {
    card.append(el('p', 'studio-failed', result.error || t('chatMediaFailed')));
  }
  card.append(el('p', 'studio-card-prompt', result.prompt));
  const actions = el('div', 'studio-actions');
  actions.append(el('span', 'studio-model', modelLabel(result.model)));
  if (result.status) {
    const stop = el('button', 'link', t('chatStop')); stop.type = 'button';
    stop.addEventListener('click', () => studio.busy?.abort());
    actions.append(stop);
  } else {
    if (result.url) {
      const download = el('a', '', t('chatDownload'));
      download.href = result.url; download.download = result.name;
      actions.append(download);
    }
    if (result.url && result.kind === 'image' && modelsFor('video').length
      && ['image/png', 'image/jpeg'].includes(result.blob.type)) {
      const use = el('button', 'link', t('chatUseImage')); use.type = 'button';
      use.disabled = !!studios.video.busy;
      use.addEventListener('click', () => {
        setReference(new File([result.blob], result.name, { type: result.blob.type }));
        workspace('video');
        studios.video.prompt.focus();
      });
      actions.append(use);
    }
    if (result.url && result.kind === 'image' && IMAGE_TYPES.includes(result.blob.type)
      && studio.inputs.length < inputLimit()) {
      const edit = el('button', 'link', t('imageUseInput')); edit.type = 'button';
      edit.disabled = !!studio.busy;
      edit.addEventListener('click', () => {
        addInputs([new File([result.blob], result.name, { type: result.blob.type })]);
        studio.prompt.focus();
      });
      actions.append(edit);
    }
    const again = el('button', 'link', t('studioReuse')); again.type = 'button';
    again.addEventListener('click', () => {
      studio.prompt.value = result.prompt;
      if ([...studio.model.options].some((option) => option.value === result.model)) studio.model.value = result.model;
      studioHint(studio);
      studio.prompt.focus();
    });
    const remove = el('button', 'link', t('studioRemove')); remove.type = 'button';
    remove.addEventListener('click', () => {
      if (result.url) confirmed(remove, t('chatSure'), () => dropResult(studio, result));
      else dropResult(studio, result);
    });
    actions.append(again, remove);
  }
  card.append(actions);
  return card;
}

function studioDraw(studio) {
  studio.list.replaceChildren(...studio.results.map((result) => studioCard(studio, result)));
  studio.empty.hidden = studio.results.length > 0;
  studio.go.textContent = t(studio.busy ? 'chatStop' : 'chatGenerate');
  studio.go.classList.toggle('stop', !!studio.busy);
  studio.model.disabled = !!studio.busy;
  if (studio.kind === 'video') {
    byId('video-start-choose').disabled = !!studio.busy;
    byId('video-start-remove').disabled = !!studio.busy;
  } else {
    drawInputs();
  }
}

// The tabs follow the models the connection offers; an open tab without models goes back to the chat.
function studioModels() {
  for (const studio of Object.values(studios)) {
    const choices = modelsFor(studio.kind).map((model) => ({ value: model.id, label: modelLabel(model.id) }));
    fillChoices(studio.model, choices);
    studio.card.update();
    byId('open-' + studio.kind).hidden = !choices.length;
    if (!choices.length && !byId(studio.kind + '-workspace').hidden && !studio.busy) workspace('chat');
    studioHint(studio);
    if (!studio.busy) studioDraw(studio);
  }
  audioModels();
}

function studioRelabel() {
  for (const studio of Object.values(studios)) {
    studio.prompt.placeholder = t(studio.kind === 'image' ? 'studioImagePlaceholder' : 'studioVideoPlaceholder');
    if (studio.problem.dataset.key) report(studio, studio.problem.dataset.key);
    studioHint(studio);
    studioDraw(studio);
  }
  audioRelabel();
}

function studioUnsaved() {
  return Object.values(studios).reduce((count, studio) =>
    count + studio.results.filter((result) => result.url || result.status).length, 0)
    + speak.results.filter((result) => result.url || result.status).length + (listen.text.value.trim() ? 1 : 0);
}

studioView('image');
studioView('video');
byId('image-input-add').addEventListener('click', () => byId('image-input-file').click());
byId('image-input-file').addEventListener('change', () => {
  const files = [...byId('image-input-file').files];
  byId('image-input-file').value = '';
  report(studios.image, '');
  addInputs(files);
});
const inputArea = byId('image-inputs');
inputArea.addEventListener('dragover', (event) => {
  if (studios.image.busy || ![...event.dataTransfer.items].some((item) => item.kind === 'file')) return;
  event.preventDefault(); inputArea.classList.add('dropping');
});
inputArea.addEventListener('dragleave', () => inputArea.classList.remove('dropping'));
inputArea.addEventListener('drop', (event) => {
  event.preventDefault(); inputArea.classList.remove('dropping');
  if (!studios.image.busy) { report(studios.image, ''); addInputs([...event.dataTransfer.files]); }
});
byId('video-start-choose').addEventListener('click', () => byId('video-start-file').click());
byId('video-start-file').addEventListener('change', () => {
  const file = byId('video-start-file').files[0];
  byId('video-start-file').value = '';
  if (file) setReference(file);
});
byId('video-start-remove').addEventListener('click', () => setReference(null));
// A picture can also be dropped onto the start image area.
const startArea = byId('video-start');
startArea.addEventListener('dragover', (event) => {
  if (studios.video.busy || ![...event.dataTransfer.items].some((item) => item.kind === 'file')) return;
  event.preventDefault(); startArea.classList.add('dropping');
});
startArea.addEventListener('dragleave', () => startArea.classList.remove('dropping'));
startArea.addEventListener('drop', (event) => {
  event.preventDefault(); startArea.classList.remove('dropping');
  const file = event.dataTransfer.files[0];
  if (file && !studios.video.busy) setReference(file);
});
window.addEventListener('pagehide', (event) => {
  for (const studio of Object.values(studios)) {
    if (studio.busy) studio.busy.abort();
    if (event.persisted) continue;
    for (const result of studio.results) if (result.url) URL.revokeObjectURL(result.url);
    if (studio.referenceUrl) URL.revokeObjectURL(studio.referenceUrl);
    for (const input of studio.inputs || []) URL.revokeObjectURL(input.url);
  }
});

/* ---- audio: text to speech and speech to text ---- */
const MAX_SPEAK_CHARS = 20000;
const speak = {
  form: byId('speak-form'), text: byId('speak-text'), model: byId('speak-model'), voice: byId('speak-voice'),
  voiceRow: byId('speak-voice-row'), go: byId('speak-go'), problem: byId('speak-problem'), list: byId('speak-results'),
  results: [], busy: null
};
const listen = {
  text: byId('listen-text'), problem: byId('listen-problem'), hint: byId('listen-file-hint'),
  choose: byId('listen-choose'), file: byId('listen-file'), drop: byId('listen-drop'), busy: null,
  // The same dictation as in a chat window, writing into this tab's text field.
  audio: chatAudio(byId('listen-text'), true)
};
const linkModel = (value) => value.startsWith('link:') ? value.slice(5) : '';
listen.card = cardButton(listen.audio.select, linkModel);
listen.audio.preference.append(listen.card.button);
byId('listen-mic').append(listen.audio.preference, listen.audio.element, listen.audio.status);
speak.card = cardButton(speak.model, linkModel);
speak.model.parentElement.append(speak.card.button);
listen.audio.select.addEventListener('focus', freshModels);
speak.model.addEventListener('focus', freshModels);
const deviceVoice = () => !!(window.speechSynthesis && window.SpeechSynthesisUtterance);

function say(target, key, text) {
  target.dataset.key = key || '';
  target.textContent = text || (key ? t(key) : '');
  target.hidden = !target.textContent;
}

function speakModels() {
  const choices = deviceVoice() ? [{ value: 'browser', label: t('chatBrowserVoice') }] : [];
  for (const model of modelsFor('speech')) {
    const label = modelLabel(model.id);
    choices.push({ value: 'link:' + model.id, label: model.local ? t('chatLocalAudio').replace('{model}', label) : label });
  }
  const linked = choices.find((choice) => choice.value.startsWith('link:'));
  fillChoices(speak.model, choices, linked?.value);
  // A Link voice gives a file; it wins until the user picks another voice.
  if (linked && !speak.model.dataset.chosen) speak.model.value = linked.value;
  speakVoices();
  speak.card.update();
  return choices.length;
}
function speakVoices() {
  const model = modelsFor('speech').find((item) => 'link:' + item.id === speak.model.value);
  const voices = model?.voices || [];
  fillChoices(speak.voice, voices.map((voice) => ({ value: voice, label: voice })));
  speak.voiceRow.hidden = !voices.length;
}
speak.model.addEventListener('change', () => { speak.model.dataset.chosen = '1'; say(speak.problem, ''); speakVoices(); });

function speakCard(result) {
  const card = el('li', 'studio-card audio-card');
  if (result.url) {
    if (!result.player) {
      result.player = el('audio', 'audio-player');
      result.player.controls = true; result.player.preload = 'metadata'; result.player.src = result.url;
    }
    card.append(result.player);
  } else if (result.status) {
    const working = el('div', 'studio-working audio-working');
    working.append(el('span', 'studio-spinner'), el('span', '', result.status));
    working.setAttribute('role', 'status');
    card.append(working);
  } else if (result.error) {
    card.append(el('p', 'studio-failed', result.error));
  }
  card.append(el('p', 'studio-card-prompt audio-card-text', result.text.length > 280 ? result.text.slice(0, 280) + ' …' : result.text));
  const actions = el('div', 'studio-actions');
  actions.append(el('span', 'studio-model', result.device ? t('speakDevice') : modelLabel(result.model) + (result.voice ? ' · ' + result.voice : '')));
  if (result.status) {
    const stop = el('button', 'link', t('chatStop')); stop.type = 'button';
    stop.addEventListener('click', () => speak.busy?.abort());
    actions.append(stop);
  } else {
    if (result.device) {
      const play = el('button', 'link', t(speechSynthesis.speaking && result.playing ? 'chatReadStop' : 'speakPlay')); play.type = 'button';
      play.addEventListener('click', () => deviceSpeak(result));
      actions.append(play);
    }
    if (result.url) {
      const download = el('a', '', t('chatDownload'));
      download.href = result.url; download.download = result.name;
      actions.append(download);
    }
    const again = el('button', 'link', t('studioReuse')); again.type = 'button';
    again.addEventListener('click', () => { speak.text.value = result.text; speak.text.focus(); });
    const remove = el('button', 'link', t('studioRemove')); remove.type = 'button';
    remove.addEventListener('click', () => {
      const drop = () => {
        if (result.playing) speechSynthesis.cancel();
        if (result.url) URL.revokeObjectURL(result.url);
        speak.results = speak.results.filter((entry) => entry !== result);
        speakDraw();
      };
      if (result.url) confirmed(remove, t('chatSure'), drop); else drop();
    });
    actions.append(again, remove);
  }
  card.append(actions);
  return card;
}
function speakDraw() {
  speak.list.replaceChildren(...speak.results.map(speakCard));
  speak.go.textContent = t(speak.busy ? 'chatStop' : 'speakGo');
  speak.go.classList.toggle('stop', !!speak.busy);
  speak.model.disabled = speak.voice.disabled = !!speak.busy;
}

// The device voice speaks sentence by sentence: some browsers cut off one long utterance.
function deviceSpeak(result) {
  const playing = result.playing;
  speechSynthesis.cancel();
  for (const entry of speak.results) entry.playing = false;
  stopReading();
  if (!playing) {
    const language = document.documentElement.lang;
    const voice = speechSynthesis.getVoices().find((item) =>
      item.localService && item.lang.split(/[-_]/)[0].toLowerCase() === language);
    if (!voice) { say(speak.problem, 'chatNoLocalVoice'); speakDraw(); return; }
    const pieces = speechPieces(result.text, 240);
    result.playing = true;
    pieces.forEach((piece, index) => {
      const utterance = new SpeechSynthesisUtterance(piece);
      utterance.voice = voice; utterance.lang = voice.lang;
      if (index === pieces.length - 1) utterance.onend = utterance.onerror = () => { result.playing = false; speakDraw(); };
      speechSynthesis.speak(utterance);
    });
  }
  speakDraw();
}

speak.text.addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); speak.form.requestSubmit(); }
});
speak.form.addEventListener('submit', async (event) => {
  event.preventDefault();
  if (speak.busy) { speak.busy.abort(); return; }
  const text = speak.text.value.trim();
  if (!text) { speak.text.focus(); return; }
  if (text.length > MAX_SPEAK_CHARS) { say(speak.problem, 'speakTooLong'); return; }
  if (!speak.model.value) { say(speak.problem, 'speakNoVoice'); return; }
  say(speak.problem, '');
  if (speak.model.value === 'browser') {
    const result = { text, device: true };
    speak.results.unshift(result);
    deviceSpeak(result);
    return;
  }
  const model = speak.model.value.slice(5), voice = speak.voiceRow.hidden ? 'alloy' : speak.voice.value;
  const result = { text, model, voice: speak.voiceRow.hidden ? '' : voice, status: t('chatMediaWaiting') };
  speak.results.unshift(result);
  const controller = new AbortController();
  speak.busy = controller;
  speakDraw();
  try {
    const blob = await speakText({ model, voice, response_format: 'wav' }, text, controller.signal,
      () => { result.status = t('chatSpeechWaiting'); speakDraw(); },
      (piece, pieces) => { if (pieces > 1) { result.status = t('speakPart').replace('{i}', piece).replace('{n}', pieces); speakDraw(); } });
    controller.signal.throwIfAborted();
    const ext = (blob.type.split(/[/;]/)[1] || 'wav').replace(/^(x-wav|wave)$/, 'wav').replace('mpeg', 'mp3');
    result.name = `${model.replace(/[^\w.-]/g, '-').slice(0, 100) || 'speech'}.${ext}`;
    result.url = URL.createObjectURL(blob);
  } catch (cause) {
    result.error = cause.name === 'AbortError' ? t('chatStopped') : cause.message || t('chatAudioFailed');
  } finally {
    delete result.status;
    speak.busy = null;
    speakDraw();
  }
});

/* Speech to text from a file: only a Link recognition model reads files. */
function listenModel() {
  const value = listen.audio.select.value;
  return value.startsWith('link:') ? value.slice(5) : '';
}
function listenState() {
  const model = listenModel();
  listen.hint.textContent = t(model ? 'listenFileHint' : 'listenNeedsLink');
  listen.choose.disabled = !model || !!listen.busy;
  listen.choose.textContent = t(listen.busy ? 'chatStop' : 'listenChoose');
  if (listen.busy) listen.choose.disabled = false;
}
listen.audio.select.addEventListener('change', listenState);
function listenAppend(text) {
  if (!text) return;
  listen.text.value += (listen.text.value && !/\s$/.test(listen.text.value) ? '\n' : '') + text;
  listen.text.dispatchEvent(new Event('input'));
}
async function listenFile(file) {
  const model = listenModel();
  if (!model || listen.busy) return;
  if (file.size > 60 * MiB) { say(listen.problem, 'audioTooLarge'); return; }
  listen.audio.cancel();
  const controller = new AbortController();
  listen.busy = controller;
  say(listen.problem, '', t('listenWorking'));
  listenState();
  try {
    const lang = document.documentElement.lang === 'de' ? 'de' : 'en';
    const text = await transcribeAudio(file, model, lang, controller.signal, (piece, pieces) => {
      if (pieces > 1) say(listen.problem, '', t('listenPart').replace('{i}', piece).replace('{n}', pieces));
    });
    controller.signal.throwIfAborted();
    if (text) { listenAppend(text); say(listen.problem, ''); } else say(listen.problem, 'chatDictateNoText');
  } catch (cause) {
    say(listen.problem, '', cause.name === 'AbortError' ? t('chatStopped') : cause.message || t('chatDictateFailed'));
  } finally {
    listen.busy = null;
    listenState();
  }
}
listen.choose.addEventListener('click', () => { if (listen.busy) listen.busy.abort(); else listen.file.click(); });
listen.file.addEventListener('change', () => {
  const file = listen.file.files[0];
  listen.file.value = '';
  if (file) listenFile(file);
});
listen.drop.addEventListener('dragover', (event) => {
  if (!listenModel() || listen.busy || ![...event.dataTransfer.items].some((item) => item.kind === 'file')) return;
  event.preventDefault(); listen.drop.classList.add('dropping');
});
listen.drop.addEventListener('dragleave', () => listen.drop.classList.remove('dropping'));
listen.drop.addEventListener('drop', (event) => {
  event.preventDefault(); listen.drop.classList.remove('dropping');
  const file = event.dataTransfer.files[0];
  if (file) listenFile(file);
});
byId('listen-copy').addEventListener('click', async (event) => {
  const button = event.currentTarget;
  if (!listen.text.value) return;
  try { await navigator.clipboard.writeText(listen.text.value); }
  catch { listen.text.select(); document.execCommand('copy'); }
  button.textContent = t('copied'); setTimeout(() => { button.textContent = t('copy'); }, 1500);
});
byId('listen-save').addEventListener('click', () => {
  if (!listen.text.value) return;
  const link = el('a');
  link.href = URL.createObjectURL(new Blob([listen.text.value], { type: 'text/plain;charset=utf-8' }));
  link.download = 'transcript.txt';
  link.click();
  setTimeout(() => URL.revokeObjectURL(link.href), 1000);
});
byId('listen-chat').addEventListener('click', () => {
  const text = listen.text.value.trim();
  if (!text) return;
  workspace('chat');
  const chat = activeChat || openChat();
  const input = chat.view.input;
  input.value += (input.value && !/\s$/.test(input.value) ? '\n' : '') + text;
  input.dispatchEvent(new Event('input'));
  input.focus();
});
byId('listen-speak').addEventListener('click', () => {
  if (!listen.text.value.trim()) return;
  speak.text.value = listen.text.value.trim();
  speak.text.focus();
});
byId('listen-clear').addEventListener('click', () => {
  if (listen.text.value) confirmed(byId('listen-clear'), t('chatSure'), () => { listen.text.value = ''; });
});

function audioModels() {
  listen.audio.relabel();
  const speaking = speakModels(), listening = listen.audio.select.options.length;
  byId('speak-form').hidden = !speaking;
  byId('listen-panel').hidden = !listening;
  byId('open-audio').hidden = !speaking && !listening;
  if (byId('open-audio').hidden && !byId('audio-workspace').hidden) workspace('chat');
  listenState();
  listen.card.update();
  if (!speak.busy) speakDraw();
}
function audioRelabel() {
  speak.text.placeholder = t('speakPlaceholder');
  listen.text.placeholder = t('listenPlaceholder');
  for (const target of [speak.problem, listen.problem]) if (target.dataset.key) say(target, target.dataset.key);
  audioModels();
}

window.addEventListener('pagehide', (event) => {
  speak.busy?.abort(); listen.busy?.abort(); listen.audio.cancel();
  if (deviceVoice()) speechSynthesis.cancel();
  if (!event.persisted) for (const result of speak.results) if (result.url) URL.revokeObjectURL(result.url);
});
studioRelabel();
studioModels();
