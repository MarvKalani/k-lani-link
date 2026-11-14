'use strict';
// Tools the chat's own model may call. The panel runs them with the connection's media
// models and shows the result in the chat; the model only gets a short note back. A video
// is made only after the user confirms it. Results stay in this page, like the Image tab's.

const CHAT_TOOL_ROUNDS = 4;
const toolLabelKeys = { create_image: 'toolImage', create_video: 'toolVideo', read_aloud: 'chatToolRead' };

function toolFunction(name, description, properties, required) {
  return { type: 'function', function: { name, description, parameters: { type: 'object', properties, required } } };
}
const textParam = (description) => ({ type: 'string', description });

// Only tools the connection can carry out right now.
function chatToolList() {
  const tools = [];
  if (modelsFor('image').length) {
    tools.push(toolFunction('create_image', 'Create an image from a description and show it to the user.',
      { prompt: textParam('What the image shows, in detail') }, ['prompt']));
  }
  if (modelsFor('video').length) {
    tools.push(toolFunction('create_video', 'Create a short video and show it to the user. The user confirms it first.',
      { prompt: textParam('What happens in the video'),
        start_from_last_image: { type: 'boolean', description: 'Start from the last image created in this conversation' } },
      ['prompt']));
  }
  if (modelsFor('speech').length || deviceVoice()) {
    tools.push(toolFunction('read_aloud', 'Read a text aloud to the user.', { text: textParam('The text to read aloud') }, ['text']));
  }
  return tools;
}
const CHAT_TOOL_HINT = 'You can create images and videos and read text aloud with your tools. When the user asks for one of these, call the tool instead of describing it.';

function toolArguments(message) {
  try { return JSON.parse(message.arguments || '{}') || {}; } catch { return {}; }
}

// Runs one tool call; the message shows its progress and gets the note for the model.
async function runChatTool(chat, message, signal) {
  const args = toolArguments(message);
  const update = (status) => { message.status = status; if (chat.view.box.isConnected) draw(chat); };
  try {
    if (message.name === 'create_image') {
      const prompt = String(args.prompt || '').trim();
      if (!prompt) throw new Error(t('chatToolNoPrompt'));
      message.model = modelsFor('image')[0].id;
      update('chatMediaWaiting');
      message.blob = await generateMedia('image', message.model, prompt, null, signal, update);
      message.kind = 'image';
    } else if (message.name === 'create_video') {
      const prompt = String(args.prompt || '').trim();
      if (!prompt) throw new Error(t('chatToolNoPrompt'));
      update('confirm');
      const yes = await new Promise((resolve, reject) => {
        message.confirm = resolve;
        signal.addEventListener('abort', () => reject(signal.reason), { once: true });
      });
      delete message.confirm;
      if (!yes) { message.declined = true; return 'The user declined to create this video.'; }
      const choice = modelsFor('video')[0];
      message.model = choice.id;
      const image = args.start_from_last_image
        ? chat.messages.findLast((entry) => entry.role === 'tool' && entry.kind === 'image' && entry.blob) : null;
      if (choice.requires_input_reference && !image) throw new Error(t('chatImageRequired'));
      const reference = image ? new File([image.blob], image.fileName, { type: image.blob.type }) : null;
      update('chatMediaWaiting');
      message.blob = await generateMedia('video', choice.id, prompt, reference, signal, update);
      message.kind = 'video';
    } else if (message.name === 'read_aloud') {
      const text = String(args.text || '').trim();
      if (!text) throw new Error(t('chatToolNoPrompt'));
      const speech = modelsFor('speech')[0];
      if (speech) {
        message.model = speech.id;
        update('chatMediaWaiting');
        message.blob = await speakText({ model: speech.id, voice: speech.voices?.[0] || 'alloy', response_format: 'wav' },
          text, signal, () => update('chatSpeechWaiting'));
        message.kind = 'audio';
      } else {
        for (const piece of speechPieces(text, 240)) speechSynthesis.speak(new SpeechSynthesisUtterance(piece));
        message.spoken = true;
      }
    } else {
      throw new Error(t('chatToolUnknown'));
    }
    if (message.blob) {
      const ext = (message.blob.type.split(/[/;]/)[1] || 'bin').replace('jpeg', 'jpg').replace(/^(x-wav|wave)$/, 'wav').replace('mpeg', 'mp3');
      message.fileName = `${(message.model || message.name).replace(/[^\w.-]/g, '-').slice(0, 100)}.${ext}`;
      message.url = URL.createObjectURL(message.blob);
    }
    return { create_image: 'The image was created and is shown to the user.',
      create_video: 'The video was created and is shown to the user.',
      read_aloud: 'The text was read aloud to the user.' }[message.name];
  } catch (cause) {
    if (cause?.name === 'AbortError' || signal.aborted) { message.error = t('chatStopped'); throw cause; }
    message.error = cause?.message || t('chatMediaFailed');
    return 'The tool failed: ' + message.error;
  } finally {
    delete message.status;
  }
}

// One tool call in the conversation: what was asked for, its progress and its result.
function toolView(chat, message) {
  const box = el('div', 'msg tool-call');
  const args = toolArguments(message);
  box.append(el('p', 'tool-call-head', t(toolLabelKeys[message.name] || 'chatToolUnknown')));
  const asked = args.prompt || args.text;
  if (asked) box.append(el('p', 'tool-call-asked', String(asked)));
  if (message.status === 'confirm') {
    const question = el('div', 'tool-call-confirm');
    question.append(el('span', '', t('chatToolConfirmVideo')));
    for (const [key, value] of [['chatToolYes', true], ['chatToolNo', false]]) {
      const button = el('button', value ? 'secondary small' : 'link', t(key)); button.type = 'button';
      button.addEventListener('click', () => message.confirm?.(value));
      question.append(button);
    }
    box.append(question);
  } else if (message.status) {
    const working = el('div', 'studio-working tool-call-working');
    working.append(el('span', 'studio-spinner'), el('span', '', t(message.status)));
    working.setAttribute('role', 'status');
    box.append(working);
  }
  if (message.url) {
    if (!message.preview) {
      const media = el(message.kind === 'image' ? 'img' : message.kind === 'video' ? 'video' : 'audio', 'tool-call-media');
      if (message.kind === 'image') media.alt = String(asked || '');
      else { media.controls = true; media.preload = 'metadata'; }
      if (message.kind === 'video') media.playsInline = true;
      media.src = message.url;
      message.preview = media;
    }
    box.append(message.preview);
    const actions = el('div', 'msg-actions');
    if (message.model) actions.append(el('span', 'media-model', modelLabel(message.model)));
    const download = el('a', '', t('chatDownload'));
    download.href = message.url; download.download = message.fileName;
    actions.append(download);
    box.append(actions);
  }
  if (message.spoken) box.append(el('p', 'msg-meta', t('chatToolSpoken')));
  if (message.declined) box.append(el('p', 'msg-meta', t('chatToolDeclined')));
  if (message.error) box.append(el('p', 'studio-failed', message.error));
  return box;
}

function releaseToolMedia(chat) {
  for (const message of chat.messages) if (message.role === 'tool' && message.url) URL.revokeObjectURL(message.url);
}
