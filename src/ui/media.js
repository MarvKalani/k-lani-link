'use strict';

const MiB = 1024 * 1024;

async function mediaRequest(path, data, signal) {
  const headers = { Authorization: 'Bearer ' + token };
  const multipart = data instanceof FormData;
  if (data && !multipart) headers['Content-Type'] = 'application/json';
  const response = await fetch('/api/media/' + path, {
    method: data ? 'POST' : 'GET', cache: 'no-store', headers, signal,
    body: data ? (multipart ? data : JSON.stringify(data)) : undefined
  });
  if (!response.ok) {
    const detail = await mediaJson(response, 64 * 1024).catch(() => ({}));
    const error = new Error(said(detail));
    error.status = response.status;
    error.gpuBusy = detail.error?.message === 'GPU memory is busy. Free the model cache and try again';
    throw error;
  }
  return response;
}

function mediaDelay(ms, signal) {
  signal.throwIfAborted();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { signal.removeEventListener('abort', cancel); resolve(); }, ms);
    const cancel = () => { clearTimeout(timer); reject(signal.reason); };
    signal.addEventListener('abort', cancel, { once: true });
  });
}

async function requestSpeech(data, signal, onWait) {
  for (let retry = 0; ; retry++) {
    try {
      return await mediaRequest('audio/speech', data, signal);
    } catch (cause) {
      // This response is emitted before the voice worker starts.
      if (cause.status !== 503 || !cause.gpuBusy || retry >= 4) throw cause;
      onWait();
      await mediaDelay(1200, signal);
    }
  }
}

async function mediaBlob(response, max) {
  const reader = response.body.getReader(), chunks = [];
  let size = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.length;
      if (size > max) throw new Error(t('chatMediaFailed'));
      chunks.push(value);
    }
  } finally {
    await reader.cancel().catch(() => {});
  }
  return new Blob(chunks, { type: response.headers.get('content-type') || '' });
}

async function mediaJson(response, max = MiB) {
  return JSON.parse(await (await mediaBlob(response, max)).text());
}

function fillChoices(select, choices, preferred = '') {
  if (choices.length === select.options.length && choices.every(({ value, label }, index) =>
    select.options[index].value === value && select.options[index].textContent === label)) return;
  const previous = select.value || preferred;
  select.replaceChildren(...choices.map(({ value, label }) => new Option(label, value)));
  if (choices.some(({ value }) => value === previous)) select.value = previous;
}

function modelsFor(capability) {
  return modelCatalog.filter((model) => model.capability === capability && model.selectable !== false);
}

// The first image of an images answer, checked to be PNG, JPEG or WebP.
async function imageFrom(response) {
  const result = await mediaJson(response, 24 * MiB);
  const encoded = result.data?.[0]?.b64_json;
  if (typeof encoded !== 'string' || !encoded) throw new Error(t('chatMediaFailed'));
  const bytes = Uint8Array.from(atob(encoded), (char) => char.charCodeAt(0));
  const signature = Array.from(bytes.slice(0, 8)).join(',');
  if (signature === '137,80,78,71,13,10,26,10') return new Blob([bytes], { type: 'image/png' });
  if (signature.startsWith('255,216,255,')) return new Blob([bytes], { type: 'image/jpeg' });
  if (signature.startsWith('82,73,70,70,') && String.fromCharCode(...bytes.slice(8, 12)) === 'WEBP') {
    return new Blob([bytes], { type: 'image/webp' });
  }
  throw new Error(t('chatMediaFailed'));
}

// `inputs`: images to edit or combine (POST /v1/images/edits); none makes a new image.
async function generateMedia(kind, model, prompt, reference, signal, status, inputs = []) {
  if (kind === 'image' && inputs.length) {
    const data = new FormData();
    data.set('model', model);
    data.set('prompt', prompt);
    data.set('response_format', 'b64_json');
    // As the OpenAI API names them: one image is `image`, several are `image[]`.
    for (const file of inputs) data.append(inputs.length > 1 ? 'image[]' : 'image', file, file.name);
    return imageFrom(await mediaRequest('images/edits', data, signal));
  }
  if (kind === 'image') {
    return imageFrom(await mediaRequest('images/generations', { model, prompt, response_format: 'b64_json' }, signal));
  }

  const data = new FormData();
  data.set('model', model);
  data.set('prompt', prompt);
  if (reference) data.set('input_reference', reference, reference.type === 'image/jpeg' ? 'start.jpg' : 'start.png');
  let job = await mediaJson(await mediaRequest('videos', data, signal));
  if (typeof job.id !== 'string' || !/^video_[A-Za-z0-9_-]{1,96}$/.test(job.id)) throw new Error(t('chatMediaFailed'));
  const path = 'videos/' + job.id;
  const deadline = Date.now() + 10 * 60 * 1000;
  while (job.status !== 'completed') {
    if (!['queued', 'in_progress'].includes(job.status)) {
      throw new Error(job.error ? said(job) : t('chatMediaFailed'));
    }
    if (Date.now() >= deadline) throw new Error(t('chatMediaTimeout'));
    status(job.status === 'queued' ? 'chatVideoQueued' : 'chatVideoWorking');
    await mediaDelay(1500, signal);
    job = await mediaJson(await mediaRequest(path, undefined, signal));
  }
  const video = await mediaBlob(await mediaRequest(path + '/content', undefined, signal), 128 * MiB);
  if (video.type.split(';')[0] !== 'video/mp4' || !video.size) throw new Error(t('chatMediaFailed'));
  return video;
}

// Records until `onReady`'s stop is called or the limit is reached.
async function recordMicrophone(signal, onReady, limits = { seconds: 90, bytes: 6 * MiB }) {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  try {
    signal.throwIfAborted();
    const recorder = new MediaRecorder(stream), chunks = [];
    return await new Promise((resolve, reject) => {
      let bytes = 0, timer;
      const stop = () => { if (recorder.state !== 'inactive') recorder.stop(); };
      const cancel = () => { stop(); reject(signal.reason); };
      const finish = () => { clearTimeout(timer); signal.removeEventListener('abort', cancel); };
      recorder.ondataavailable = ({ data }) => {
        bytes += data.size;
        if (bytes > limits.bytes) { stop(); reject(new Error(t('chatRecordingLimit'))); }
        else if (data.size) chunks.push(data);
      };
      recorder.onstop = () => { finish(); resolve(new Blob(chunks, { type: recorder.mimeType })); };
      recorder.onerror = () => { finish(); stop(); reject(new Error(t('chatDictateFailed'))); };
      recorder.start(1000);
      timer = setTimeout(stop, limits.seconds * 1000);
      signal.addEventListener('abort', cancel, { once: true });
      onReady(stop);
    });
  } finally {
    for (const track of stream.getTracks()) track.stop();
  }
}

const SPEECH_RATE = 16000;
// Longest recording or file the page turns into text; it is cut into pieces of at most a minute.
const MAX_AUDIO_SECONDS = 15 * 60;
const PIECE_SECONDS = 60;

async function decodeAudio(blob) {
  const context = new AudioContext();
  try {
    return await context.decodeAudioData(await blob.arrayBuffer());
  } catch {
    throw new Error(t('audioUnreadable'));
  } finally {
    await context.close();
  }
}

// Mono at the rate speech recognition expects.
async function speechSamples(decoded) {
  const length = Math.max(1, Math.ceil(decoded.duration * SPEECH_RATE));
  const mixer = new OfflineAudioContext(1, length, SPEECH_RATE);
  const source = mixer.createBufferSource();
  source.buffer = decoded;
  source.connect(mixer.destination);
  source.start();
  return (await mixer.startRendering()).getChannelData(0);
}

function wavFile(samples, rate) {
  const wav = new DataView(new ArrayBuffer(44 + samples.length * 2));
  const ascii = (at, text) => Array.from(text).forEach((char, offset) => wav.setUint8(at + offset, char.charCodeAt(0)));
  ascii(0, 'RIFF'); wav.setUint32(4, wav.byteLength - 8, true); ascii(8, 'WAVEfmt ');
  wav.setUint32(16, 16, true); wav.setUint16(20, 1, true); wav.setUint16(22, 1, true);
  wav.setUint32(24, rate, true); wav.setUint32(28, rate * 2, true);
  wav.setUint16(32, 2, true); wav.setUint16(34, 16, true);
  ascii(36, 'data'); wav.setUint32(40, samples.length * 2, true);
  for (let i = 0; i < samples.length; i++) wav.setInt16(44 + i * 2, Math.max(-1, Math.min(1, samples[i])) * 32767, true);
  return new Blob([wav.buffer], { type: 'audio/wav' });
}

// Where to cut: the quietest tenth of a second in the last eight seconds before each limit,
// so a cut rarely splits a word.
function quietCuts(samples, rate) {
  const cuts = [0], span = Math.round(rate / 10);
  let start = 0;
  while (samples.length - start > PIECE_SECONDS * rate) {
    const end = start + PIECE_SECONDS * rate;
    let best = end, quietest = Infinity;
    for (let at = end - 8 * rate; at + span <= end; at += span) {
      let energy = 0;
      for (let i = at; i < at + span; i++) energy += samples[i] * samples[i];
      if (energy < quietest) { quietest = energy; best = at + (span >> 1); }
    }
    cuts.push(best);
    start = best;
  }
  cuts.push(samples.length);
  return cuts;
}

// Any audio the browser can decode, up to fifteen minutes, piece by piece.
async function transcribeAudio(recording, model, language, signal, progress = () => {}) {
  const decoded = await decodeAudio(recording);
  if (decoded.duration > MAX_AUDIO_SECONDS) throw new Error(t('audioTooLong'));
  signal.throwIfAborted();
  const samples = await speechSamples(decoded);
  const cuts = quietCuts(samples, SPEECH_RATE), texts = [];
  for (let piece = 1; piece < cuts.length; piece++) {
    progress(piece, cuts.length - 1);
    const data = new FormData();
    data.set('model', model);
    data.set('language', language);
    data.set('file', wavFile(samples.subarray(cuts[piece - 1], cuts[piece]), SPEECH_RATE), 'speech.wav');
    signal.throwIfAborted();
    const result = await mediaJson(await mediaRequest('audio/transcriptions', data, signal));
    if (typeof result.text === 'string' && result.text.trim()) texts.push(result.text.trim());
  }
  return texts.join(' ');
}

// Sentences packed into pieces a speech service takes at once; a sentence longer than a
// piece is cut at a space.
function speechPieces(text, max = 1000) {
  const pieces = [];
  let current = '';
  for (const sentence of text.match(/[^.!?…\n]+[.!?…]*\s*|\n+/g) || []) {
    let rest = sentence;
    while (rest.length > max) {
      const space = rest.lastIndexOf(' ', max);
      const at = space > max / 2 ? space : max;
      if (current.trim()) { pieces.push(current.trim()); current = ''; }
      pieces.push(rest.slice(0, at).trim());
      rest = rest.slice(at);
    }
    if ((current + rest).length > max && current.trim()) { pieces.push(current.trim()); current = ''; }
    current += rest;
  }
  if (current.trim()) pieces.push(current.trim());
  return pieces;
}

// One audio file for the whole text: each piece is asked for in turn, then joined.
async function speakText(request, text, signal, onWait, progress = () => {}) {
  const pieces = speechPieces(text), blobs = [];
  for (let piece = 0; piece < pieces.length; piece++) {
    progress(piece + 1, pieces.length);
    const blob = await mediaBlob(await requestSpeech({ ...request, input: pieces[piece] }, signal, onWait), 32 * MiB);
    if (!/^audio\/(wav|wave|x-wav|mpeg|ogg|mp4|flac)(?:;|$)/.test(blob.type) || !blob.size) throw new Error(t('chatAudioFailed'));
    blobs.push(blob);
  }
  if (blobs.length === 1) return blobs[0];
  const context = new AudioContext();
  try {
    const parts = [];
    for (const blob of blobs) parts.push((await context.decodeAudioData(await blob.arrayBuffer())).getChannelData(0));
    const joined = new Float32Array(parts.reduce((sum, part) => sum + part.length, 0));
    let at = 0;
    for (const part of parts) { joined.set(part, at); at += part.length; }
    return wavFile(joined, context.sampleRate);
  } finally {
    await context.close();
  }
}

/* ---- model cards ---- */
const readmes = new Map();
function modelCard(id) {
  return modelCatalog.find((model) => model.id === id)?.card || null;
}
// A "model card" button for a model list; hidden while the chosen model has none.
function cardButton(select, idOf = (value) => value) {
  const button = el('button', 'card-button');
  button.type = 'button';
  const update = () => {
    button.hidden = !modelCard(idOf(select.value));
    button.textContent = t('cardOpen');
    button.title = t('cardHint');
  };
  select.addEventListener('change', update);
  button.addEventListener('click', () => showCard(idOf(select.value)));
  update();
  return { button, update };
}
async function showCard(id) {
  const card = modelCard(id);
  if (!card) return;
  const dialog = byId('model-card');
  byId('model-card-title').textContent = modelLabel(id);
  for (const [name, link] of [['page', card.page], ['benchmarks', card.benchmarks]]) {
    const anchor = byId('model-card-' + name);
    anchor.hidden = !link;
    if (link) anchor.href = link; else anchor.removeAttribute('href');
  }
  const text = byId('model-card-readme'), status = byId('model-card-status');
  text.textContent = '';
  text.hidden = true;
  status.textContent = card.readme ? t('cardLoading') : (card.page || card.benchmarks ? '' : t('cardEmpty'));
  if (!dialog.open) dialog.showModal();
  if (!card.readme) return;
  try {
    if (!readmes.has(card.readme)) readmes.set(card.readme, (await chatApi('GET', 'model-card/' + card.readme)).readme);
    if (byId('model-card-title').textContent !== modelLabel(id)) return;
    // The README is shown as plain text, never as markup.
    text.textContent = readmes.get(card.readme);
    text.hidden = !text.textContent;
    status.textContent = '';
  } catch (cause) {
    status.textContent = cause.message || t('cardFailed');
  }
}
byId('model-card-close').addEventListener('click', () => byId('model-card').close());
byId('model-card').addEventListener('click', (event) => {
  if (event.target === byId('model-card')) byId('model-card').close();
});
