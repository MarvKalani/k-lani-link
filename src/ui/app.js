'use strict';
const byId = (id) => document.getElementById(id);
const bootstrap = location.hash.slice(1);
if (bootstrap) { sessionStorage.setItem('link-control-token', bootstrap); history.replaceState(null, '', '/'); }
const token = sessionStorage.getItem('link-control-token') || '';
const texts = {
  en: {
    step1: 'Step 1 of 3', step2: 'Step 2 of 3', step3: 'Step 3 of 3',
    pairTitle: 'Connect to the model computer',
    pairIntro: 'The model computer shows a relay address and a pairing code. Type both in here.',
    relay: 'Relay address', change: 'change', code: 'Pairing code', connect: 'Connect',
    pairNote: 'A code works once and for ten minutes. If it fails, ask for a new one.',
    or: 'or', qrIntro: 'Does the model computer show a QR code? Scan it with this computer’s camera.',
    qrCamera: 'Scan QR code', qrPicture: 'Use a picture instead',
    qrHold: 'Hold the QR code in front of the camera. Pairing starts by itself.',
    qrPermission: 'The browser asks for the camera each time k-lani-link starts. That is normal.',
    qrDenied: 'The camera was not allowed. Allow it in the browser, or choose a picture of the QR code.',
    qrNoCamera: 'No camera found. Choose a picture of the QR code instead, e.g. a screenshot.',
    qrNotFound: 'No QR code found in this picture.',
    updateNow: 'Update now', updateReady: 'Version {v} is ready and is installed at the next start.',
    updateLoading: 'Version {v} is available and is being downloaded.',
    updateManual: 'Version {v} is available, but this program’s folder is read-only. Download it from',
    updateSure: '{n} unsaved chat window(s) will close. Click again to update',
    updateTitle: 'Updates', updateAuto: 'Look for new versions by itself',
    updateInfo: 'Twice a day, k-lani-link asks GitHub whether a newer version exists; only its own version number goes along. A new version must carry the k-lani-link signature. It is downloaded and installed at the next start.',
    sUpdating: 'Updating…', updatingTitle: 'Updating',
    updatingDetail: 'k-lani-link restarts with the new version and opens a new tab. You can close this one.',
    cancel: 'Cancel', readyTitle: 'Connected',
    readyIntro: 'In your AI app, add an “OpenAI-compatible” provider and copy in these three values:',
    base: 'Base URL', key: 'API key', model: 'Model', copy: 'Copy', copied: 'Copied',
    readyNote: 'These values only work on this computer. You can close this page; the connection stays on.',
    readyNoteTray: 'These values only work on this computer. You can close this page: k-lani-link stays in the notification area of the taskbar (bottom right), where you can also pause or quit it.',
    more: 'More options and help', repair: 'Pair again with a new code', disconnect: 'Disconnect', quit: 'Quit k-lani-link',
    helpTitle: 'If it does not connect',
    help1: 'Waiting a long time: k-lani-link must be running on the model computer (“host”).',
    help2: 'Code not accepted: each code works once. Ask for a new one.',
    help3: 'Access refused: the access for this computer expired or was withdrawn. Ask for a new code.',
    help4: 'No model: a model must be loaded on the model computer.',
    help5: 'Port occupied: another k-lani-link client is already running on this computer.',
    fileTitle: 'Connect with a client.toml file instead',
    fileNote: 'Only if the operator gave you a file. Keep it out of Git, chats and screenshots.',
    fileChoose: 'Choose client.toml', fileTooLarge: 'This file is too large.',
    docker: 'Using OpenCode in Docker? It has its own launcher and does not use this page.', dockerLink: 'Docker setup',
    privacy: 'This page controls the connection. A chat is kept only while its window is open, unless you save it.',
    chatTitle: 'Chat with the model', chatIntro: 'Try the model right here. Every window is its own conversation.',
    chatNew: 'New chat', chatSavedTitle: 'Saved chats', chatWhere: 'Saved on this computer only, in:',
    chatMessages: 'messages', chatDelete: 'Delete', chatSure: 'Really delete?', chatUntitled: 'New chat',
    chatSave: 'Save', chatIsSaved: 'Saved', chatUnsave: 'Delete saved copy?',
    chatSaveHint: 'Keep this chat on this computer', chatUnsaveHint: 'Click twice to delete the saved copy',
    chatClose: 'Close', chatLoose: 'Not saved: closing this window ends the conversation.',
    chatKept: 'Saved on this computer. Every answer is saved as well.',
    chatModel: 'Model', chatEffort: 'Thinking', chatDefaultModel: 'Default model',
    effortAuto: 'Default', effort_none: 'Off', effort_low: 'Low', effort_medium: 'Medium', effort_high: 'High',
    chatPlaceholder: 'Message … (Enter sends, Shift+Enter for a new line)', chatSend: 'Send', chatStop: 'Stop',
    chatRetry: 'Retry', chatThinking: 'Thinking', chatFirst: 'first token', chatRead: 'reading', chatCache: 'cache', chatContext: 'context',
    chatStopped: 'stopped', chatLength: 'length limit reached', chatWriting: 'writing …', chatWaiting: 'waiting for the model computer …', chatCut: 'The connection was interrupted. Try again.',
    sUnpaired: 'Not paired', sPairing: 'Pairing…', sConnecting: 'Connecting…', sConnected: 'Connected',
    sRecovering: 'Reconnecting…', sProblem: 'Not connected', sOffline: 'Not running', sClosed: 'Closed',
    waitPairTitle: 'Waiting for the model computer…',
    waitPairDetail: 'Keep this page open. This takes a few seconds.',
    waitConnTitle: 'Connecting…',
    waitConnDetail: 'k-lani-link must be running on the model computer (“host”). This page continues by itself.',
    waitRecTitle: 'Connection interrupted, reconnecting…',
    waitRecDetail: 'This page continues by itself. A reply that was interrupted is not repeated.',
    disconnectedTitle: 'Not connected', disconnectedDetail: 'Make sure k-lani-link is running on the model computer, then connect.',
    stoppedTitle: 'The connection stopped', stoppedDetail: 'Try again. If it keeps stopping, another k-lani-link may be running on this computer.',
    deniedTitle: 'Access refused', deniedDetail: 'The access for this computer expired or was withdrawn. Ask for a new code.',
    rejectedTitle: 'The other side could not be verified', rejectedDetail: 'Ask for a new code and pair again.',
    usedTitle: 'A new code is needed', usedDetail: 'This pairing is used up. Ask for a new code.',
    fixConnect: 'Connect', fixReconnect: 'Try again', fixNewCode: 'Enter a new code',
    needCode: 'Enter the pairing code.', needRelay: 'Enter the relay address.',
    noModel: 'No model is loaded on the model computer yet. This page checks again by itself.',
    modelFail: 'The model computer does not answer yet:',
    offlineDetail: 'k-lani-link is not running. Open it again to continue.', closedDetail: 'k-lani-link is closing. You can close this tab.',
    unexpected: 'k-lani-link returned an unexpected answer.'
  },
  de: {
    step1: 'Schritt 1 von 3', step2: 'Schritt 2 von 3', step3: 'Schritt 3 von 3',
    pairTitle: 'Mit dem Modellrechner verbinden',
    pairIntro: 'Der Modellrechner zeigt eine Relay-Adresse und einen Kopplungscode. Gib beides hier ein.',
    relay: 'Relay-Adresse', change: 'ändern', code: 'Kopplungscode', connect: 'Verbinden',
    pairNote: 'Ein Code gilt einmal und zehn Minuten. Klappt es nicht, lass dir einen neuen geben.',
    or: 'oder', qrIntro: 'Zeigt der Modellrechner einen QR-Code? Scanne ihn mit der Kamera dieses Computers.',
    qrCamera: 'QR-Code scannen', qrPicture: 'Stattdessen ein Bild wählen',
    qrHold: 'Halte den QR-Code vor die Kamera. Die Kopplung startet von selbst.',
    qrPermission: 'Der Browser fragt bei jedem Start von k-lani-link nach der Kamera. Das ist normal.',
    qrDenied: 'Die Kamera wurde nicht erlaubt. Erlaube sie im Browser oder wähle ein Bild des QR-Codes.',
    qrNoCamera: 'Keine Kamera gefunden. Wähle stattdessen ein Bild des QR-Codes, z. B. ein Bildschirmfoto.',
    qrNotFound: 'In diesem Bild ist kein QR-Code zu finden.',
    updateNow: 'Jetzt aktualisieren', updateReady: 'Version {v} ist bereit und wird beim nächsten Start installiert.',
    updateLoading: 'Version {v} ist verfügbar und wird geladen.',
    updateManual: 'Version {v} ist verfügbar, aber der Programmordner ist schreibgeschützt. Lade sie herunter von',
    updateSure: '{n} ungespeicherte Chatfenster gehen zu. Zum Aktualisieren nochmal klicken',
    updateTitle: 'Updates', updateAuto: 'Selbst nach neuen Versionen suchen',
    updateInfo: 'Zweimal am Tag fragt k-lani-link bei GitHub, ob es eine neuere Version gibt; mitgeschickt wird nur die eigene Versionsnummer. Eine neue Version muss die k-lani-link-Signatur tragen. Sie wird geladen und beim nächsten Start installiert.',
    sUpdating: 'Aktualisiere…', updatingTitle: 'Wird aktualisiert',
    updatingDetail: 'k-lani-link startet mit der neuen Version neu und öffnet einen neuen Tab. Diesen kannst du schließen.',
    cancel: 'Abbrechen', readyTitle: 'Verbunden',
    readyIntro: 'Lege in deiner KI-App einen „OpenAI-kompatiblen“ Anbieter an und kopiere diese drei Werte hinein:',
    base: 'Basis-URL', key: 'API-Schlüssel', model: 'Modell', copy: 'Kopieren', copied: 'Kopiert',
    readyNote: 'Diese Werte funktionieren nur auf diesem Computer. Du kannst die Seite schließen, die Verbindung bleibt bestehen.',
    readyNoteTray: 'Diese Werte funktionieren nur auf diesem Computer. Du kannst die Seite schließen: k-lani-link bleibt unten rechts im Infobereich der Taskleiste. Dort kannst du es auch pausieren oder beenden.',
    more: 'Weitere Optionen und Hilfe', repair: 'Mit neuem Code neu koppeln', disconnect: 'Trennen', quit: 'k-lani-link beenden',
    helpTitle: 'Wenn es nicht verbindet',
    help1: 'Es dauert lange: Auf dem Modellrechner muss k-lani-link laufen („host“).',
    help2: 'Code nicht angenommen: Jeder Code gilt nur einmal. Lass dir einen neuen geben.',
    help3: 'Zugang verweigert: Der Zugang dieses Computers ist abgelaufen oder entzogen. Lass dir einen neuen Code geben.',
    help4: 'Kein Modell: Auf dem Modellrechner muss ein Modell geladen sein.',
    help5: 'Port belegt: Auf diesem Computer läuft schon ein anderer k-lani-link-Client.',
    fileTitle: 'Stattdessen mit einer client.toml-Datei verbinden',
    fileNote: 'Nur wenn du vom Betreiber eine Datei bekommen hast. Nicht in Git, Chats oder Screenshots ablegen.',
    fileChoose: 'client.toml auswählen', fileTooLarge: 'Diese Datei ist zu groß.',
    docker: 'OpenCode in Docker? Das hat einen eigenen Starter und nutzt diese Seite nicht.', dockerLink: 'Docker-Anleitung',
    privacy: 'Diese Seite steuert die Verbindung. Ein Chat bleibt nur, solange sein Fenster offen ist, außer du speicherst ihn.',
    chatTitle: 'Chat mit dem Modell', chatIntro: 'Probiere das Modell direkt hier aus. Jedes Fenster ist ein eigenes Gespräch.',
    chatNew: 'Neuer Chat', chatSavedTitle: 'Gespeicherte Chats', chatWhere: 'Nur auf diesem Computer gespeichert, in:',
    chatMessages: 'Nachrichten', chatDelete: 'Löschen', chatSure: 'Wirklich löschen?', chatUntitled: 'Neuer Chat',
    chatSave: 'Speichern', chatIsSaved: 'Gespeichert', chatUnsave: 'Gespeicherte Kopie löschen?',
    chatSaveHint: 'Diesen Chat auf diesem Computer behalten', chatUnsaveHint: 'Zweimal klicken löscht die gespeicherte Kopie',
    chatClose: 'Schließen', chatLoose: 'Nicht gespeichert: Mit dem X ist dieses Gespräch weg.',
    chatKept: 'Auf diesem Computer gespeichert. Jede Antwort wird mitgespeichert.',
    chatModel: 'Modell', chatEffort: 'Denken', chatDefaultModel: 'Standardmodell',
    effortAuto: 'Standard', effort_none: 'Aus', effort_low: 'Wenig', effort_medium: 'Mittel', effort_high: 'Viel',
    chatPlaceholder: 'Nachricht … (Enter sendet, Umschalt+Enter für neue Zeile)', chatSend: 'Senden', chatStop: 'Stopp',
    chatRetry: 'Nochmal', chatThinking: 'Denkprozess', chatFirst: 'erstes Token', chatRead: 'Lesen', chatCache: 'Cache', chatContext: 'Kontext',
    chatStopped: 'abgebrochen', chatLength: 'Längengrenze erreicht', chatWriting: 'schreibt …', chatWaiting: 'wartet auf den Modellrechner …', chatCut: 'Die Verbindung wurde unterbrochen. Versuche es erneut.',
    sUnpaired: 'Nicht gekoppelt', sPairing: 'Koppeln…', sConnecting: 'Verbinde…', sConnected: 'Verbunden',
    sRecovering: 'Verbinde neu…', sProblem: 'Nicht verbunden', sOffline: 'Läuft nicht', sClosed: 'Beendet',
    waitPairTitle: 'Warte auf den Modellrechner…',
    waitPairDetail: 'Lass diese Seite offen. Das dauert ein paar Sekunden.',
    waitConnTitle: 'Verbinde…',
    waitConnDetail: 'Auf dem Modellrechner muss k-lani-link laufen („host“). Die Seite macht von selbst weiter.',
    waitRecTitle: 'Verbindung unterbrochen, verbinde neu…',
    waitRecDetail: 'Die Seite macht von selbst weiter. Eine unterbrochene Antwort wird nicht wiederholt.',
    disconnectedTitle: 'Nicht verbunden', disconnectedDetail: 'Prüfe, dass k-lani-link auf dem Modellrechner läuft, und verbinde dann.',
    stoppedTitle: 'Die Verbindung wurde beendet', stoppedDetail: 'Versuche es noch einmal. Bricht sie immer wieder ab, läuft auf diesem Computer vielleicht schon ein anderes k-lani-link.',
    deniedTitle: 'Zugang verweigert', deniedDetail: 'Der Zugang dieses Computers ist abgelaufen oder entzogen. Lass dir einen neuen Code geben.',
    rejectedTitle: 'Die Gegenseite ließ sich nicht prüfen', rejectedDetail: 'Lass dir einen neuen Code geben und kopple neu.',
    usedTitle: 'Ein neuer Code ist nötig', usedDetail: 'Diese Kopplung ist verbraucht. Lass dir einen neuen Code geben.',
    fixConnect: 'Verbinden', fixReconnect: 'Noch einmal versuchen', fixNewCode: 'Neuen Code eingeben',
    needCode: 'Gib den Kopplungscode ein.', needRelay: 'Gib die Relay-Adresse ein.',
    noModel: 'Auf dem Modellrechner ist noch kein Modell geladen. Die Seite prüft von selbst erneut.',
    modelFail: 'Der Modellrechner antwortet noch nicht:',
    offlineDetail: 'k-lani-link läuft nicht. Öffne es erneut, um weiterzumachen.', closedDetail: 'k-lani-link wird beendet. Du kannst diesen Tab schließen.',
    unexpected: 'k-lani-link hat unerwartet geantwortet.'
  }
};
// The application answers in English; these are the messages a panel user meets.
const serverDe = {
  'A pairing code has eleven digits, e.g. 472-1234-5678.': 'Ein Kopplungscode hat elf Ziffern, z. B. 472-1234-5678.',
  'Enter the pairing code.': 'Gib den Kopplungscode ein.',
  'Enter the relay address.': 'Gib die Relay-Adresse ein.',
  'This code is unknown, expired or already used. Ask for a new code.': 'Dieser Code ist unbekannt, abgelaufen oder schon benutzt. Lass dir einen neuen geben.',
  'The code was not accepted. Check the digits and ask for a new code.': 'Der Code wurde nicht angenommen. Prüfe die Ziffern und lass dir einen neuen geben.',
  'The other side could not be verified. Ask for a new code.': 'Die Gegenseite ließ sich nicht prüfen. Lass dir einen neuen Code geben.',
  'The other computer did not confirm the pairing. Create a new code.': 'Der Modellrechner hat die Kopplung nicht bestätigt. Lass dir einen neuen Code geben.',
  'The pairing code expired or was already used. Create a new code.': 'Der Code ist abgelaufen oder schon benutzt. Lass dir einen neuen geben.',
  'This is not a valid pairing code.': 'Das ist kein gültiger Kopplungscode.',
  'Pairing failed. Ask for a new code and try again.': 'Die Kopplung ist fehlgeschlagen. Lass dir einen neuen Code geben und versuche es erneut.',
  'The relay cannot be reached. Check the relay address and the network.': 'Das Relay ist nicht erreichbar. Prüfe die Relay-Adresse und die Internetverbindung.',
  'The relay holds too many open pairing codes. Wait ten minutes or use an open one.': 'Auf dem Relay sind zu viele Codes offen. Warte zehn Minuten oder nimm einen offenen Code.',
  'Too many failed attempts from this address. Wait a few minutes and try again.': 'Zu viele Fehlversuche von dieser Adresse. Warte ein paar Minuten und versuche es erneut.',
  'This relay cannot pair by code. Update it, or copy the client folder from init-https.': 'Dieses Relay kann nicht per Code koppeln. Frag den Betreiber nach einer client.toml-Datei.',
  'The local API port is occupied. Stop the other client or import a configuration with another loopback port.': 'Der lokale Port ist belegt. Beende den anderen k-lani-link-Client auf diesem Computer.',
  'The encrypted tunnel is not connected yet. Check the remote host, pairing code and network permissions.': 'Die verschlüsselte Verbindung steht noch nicht.',
  'The backend did not respond before the test timeout.': 'Das Modell hat nicht rechtzeitig geantwortet.',
  'The tunnel is connected, but the backend rejected model discovery. Check its loaded model and required privacy profile.': 'Die Verbindung steht, aber der Modellrechner liefert keine Modellliste. Ist dort ein Modell geladen?',
  'This is not a valid client.toml configuration.': 'Das ist keine gültige client.toml-Datei.',
  'This QR code is not a k-lani-link pairing code. Show the code from offer-https --qr.': 'Dieser QR-Code ist kein k-lani-link-Kopplungscode. Zeige den Code von offer-https --qr.',
  'This QR code is not a valid k-lani-link pairing code.': 'Dieser QR-Code ist kein gültiger k-lani-link-Kopplungscode.',
  'This is not a camera picture.': 'Das ist kein Kamerabild.',
  'Disconnect before pairing again.': 'Trenne zuerst die Verbindung, dann koppeln.',
  'No update is ready yet.': 'Es ist noch kein Update bereit.',
  'Unknown update action.': 'Unbekannte Update-Aktion.',
  'The setting could not be saved.': 'Die Einstellung ließ sich nicht speichern.',
  'A pairing is already waiting.': 'Eine Kopplung läuft bereits.',
  'Start the connection first.': 'Stelle zuerst die Verbindung her.',
  'This is not a valid chat request.': 'Diese Chat-Anfrage ist ungültig.',
  'The connection did not answer. Try again.': 'Die Verbindung hat nicht geantwortet. Versuche es erneut.',
  'Too many saved chats. Delete some first.': 'Zu viele gespeicherte Chats. Lösche zuerst einige.',
  'The chat could not be saved.': 'Der Chat ließ sich nicht speichern.',
  'The chat could not be deleted.': 'Der Chat ließ sich nicht löschen.',
  'This chat does not exist.': 'Diesen Chat gibt es nicht.',
  'request capacity exhausted': 'Der Modellrechner ist gerade ausgelastet. Versuche es gleich noch einmal.',
  'home peer is not connected': 'Die Verbindung zum Modellrechner steht gerade nicht.',
  'peer disconnected': 'Die Verbindung zum Modellrechner wurde unterbrochen.',
  'upstream response timed out': 'Der Modellrechner hat nicht rechtzeitig geantwortet.',
  'request body unavailable or too large': 'Dieser Chat ist zu lang. Starte einen neuen Chat.'
};
const language = (navigator.languages || [navigator.language || 'en']).some((tag) => /^de\b/i.test(tag)) ? 'de' : 'en';
const t = (key) => texts[language][key] || texts.en[key] || key;
document.documentElement.lang = language;
for (const element of document.querySelectorAll('[data-t]')) element.textContent = t(element.dataset.t);
let closed = false;
let busy = false;
let current = null;
let relayChosen = false;
let wantPair = false;
let fix = null;
let apiKey = '';
let models = null;
let lastTest = 0;
let checking = false;
async function api(path, data) {
  const response = await fetch('/api/' + path, {
    method: data === undefined ? 'GET' : 'POST',
    headers: { Authorization: 'Bearer ' + token, ...(data === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: data === undefined ? undefined : JSON.stringify(data), cache: 'no-store'
  });
  const value = await response.json().catch(() => ({ error: t('unexpected') }));
  if (!response.ok) throw new Error((language === 'de' && serverDe[value.error]) || value.error || t('unexpected'));
  return value;
}
function error(message) { byId('error').textContent = message; byId('error').hidden = !message; }
function show(step) { for (const id of ['pair', 'wait', 'ready', 'problem']) byId('step-' + id).hidden = id !== step; }
const icons = { connected: 'connected', joining: 'waiting', connecting: 'waiting', recovering: 'waiting', rejected: 'problem', offline: 'problem' };
function light(phase, label) {
  byId('light').className = 'light ' + phase; byId('status').textContent = label; document.title = label + ' · k-lani-link';
  const icon = '/icon/' + (icons[phase] || 'idle') + '.png';
  if (byId('favicon').getAttribute('href') !== icon) byId('favicon').setAttribute('href', icon);
}
function wait(title, detail, cancel) {
  byId('wait-title').textContent = t(title); byId('wait-detail').textContent = t(detail); byId('cancel').hidden = !cancel; show('wait');
}
function problem(name, button, work) {
  byId('problem-title').textContent = t(name + 'Title'); byId('problem-detail').textContent = t(name + 'Detail');
  byId('fix').textContent = t(button); fix = work; show('problem');
}
function forget() { apiKey = ''; models = null; lastTest = 0; byId('key').value = '••••••••••••'; byId('key').type = 'password'; }
function render(state) {
  current = state;
  byId('version').textContent = state.version ? 'v' + state.version : '';
  if (state.default_relay && !relayChosen) {
    byId('relay-name').textContent = state.default_relay; byId('relay-preset').hidden = false; byId('relay-row').hidden = true;
  }
  if (state.api_base) byId('base').value = state.api_base;
  byId('ready-note').textContent = t(state.tray ? 'readyNoteTray' : 'readyNote');
  showUpdate(state.update);
  byId('disconnect').hidden = !state.running;
  byId('repair').hidden = !state.configured || state.joining;
  byId('config').disabled = busy || state.running || state.joining;
  for (const id of ['pair', 'fix', 'cancel', 'repair', 'disconnect', 'quit']) byId(id).disabled = busy;
  if (state.phase !== 'connected') forget();
  if (state.joining) { light('joining', t('sPairing')); wait('waitPairTitle', 'waitPairDetail', false); return; }
  if (!state.configured || (wantPair && !state.running)) {
    light('', t('sUnpaired'));
    if (byId('step-pair').hidden) { show('pair'); byId(byId('relay-row').hidden || byId('relay').value ? 'code' : 'relay').focus(); }
    return;
  }
  switch (state.phase) {
    case 'connected': light('connected', t('sConnected')); show('ready'); ready(); return;
    case 'connecting': light('connecting', t('sConnecting')); wait('waitConnTitle', 'waitConnDetail', true); return;
    case 'recovering': light('recovering', t('sRecovering')); wait('waitRecTitle', 'waitRecDetail', true); return;
    case 'disconnected': light('', t('sProblem')); problem('disconnected', 'fixConnect', () => api('connect', {})); return;
    case 'stopped':
      light('rejected', t('sProblem'));
      problem('stopped', 'fixReconnect', async () => { await api('disconnect', {}); await api('connect', {}); });
      return;
    case 'denied': case 'rejected': case 'new_code_required':
      light('rejected', t('sProblem'));
      problem({ denied: 'denied', rejected: 'rejected', new_code_required: 'used' }[state.phase], 'fixNewCode', newCode);
      return;
    default: light('', t('sProblem')); problem('stopped', 'fixReconnect', async () => { await api('disconnect', {}); await api('connect', {}); });
  }
}
// A downloaded update, and the setting that lets the panel look for one.
function showUpdate(update) {
  const supported = !!(update && update.supported);
  byId('update-setting').hidden = !supported;
  if (!supported) { byId('update-note').hidden = true; return; }
  byId('update-auto').checked = update.enabled;
  const version = update.available || '';
  const key = !version ? '' : update.staged ? 'updateReady' : update.writable ? 'updateLoading' : 'updateManual';
  byId('update-note').hidden = !key;
  byId('update-text').textContent = key ? t(key).replace('{v}', version) : '';
  byId('update-link').hidden = key !== 'updateManual';
  byId('update-now').hidden = key !== 'updateReady';
}
async function newCode() { if (current && current.running) await api('disconnect', {}); wantPair = true; byId('code').value = ''; }
// Fetches what the AI app needs once per connection; retries the model list while none is loaded.
async function ready() {
  if (busy || checking) return;
  checking = true;
  try { await check(); } finally { checking = false; }
  chatsReady();
}
async function check() {
  if (!apiKey) {
    try { apiKey = (await api('credentials', {})).api_key; byId('key').value = apiKey; } catch { return; }
  }
  if (models && models.length) return;
  if (Date.now() - lastTest < 10000) return;
  lastTest = Date.now();
  try {
    models = (await api('test', {})).models;
    byId('model-problem').textContent = models.length ? '' : t('noModel');
  } catch (cause) {
    models = null; byId('model-problem').textContent = t('modelFail') + ' ' + cause.message;
  }
  byId('model-problem').hidden = !byId('model-problem').textContent;
  const list = models || [];
  const select = byId('model');
  select.replaceChildren(...list.map((id) => new Option(id, id)));
  select.hidden = list.length < 2; byId('model-one').hidden = list.length > 1;
  byId('model-one').value = list[0] || '';
  chatModels();
}
async function refresh() {
  if (closed) return;
  try { render(await api('status')); }
  catch {
    light('offline', t('sOffline')); byId('problem-title').textContent = t('sOffline');
    byId('problem-detail').textContent = t('offlineDetail'); byId('fix').hidden = true; show('problem');
    return;
  }
  byId('fix').hidden = false;
}
async function action(work) {
  if (busy || closed) return;
  busy = true; error(''); if (current) render(current);
  try { await work(); } catch (cause) { error(cause.message || t('unexpected')); }
  finally { busy = false; await refresh(); }
}
function relayAddress() {
  if (!byId('relay-row').hidden) {
    const relay = byId('relay').value.trim();
    if (!relay) throw new Error(t('needRelay'));
    return /^https?:\/\//i.test(relay) ? relay : 'https://' + relay;
  }
  return '';
}
byId('code').addEventListener('input', () => {
  const digits = byId('code').value.replace(/\D/g, '').slice(0, 11);
  byId('code').value = [digits.slice(0, 3), digits.slice(3, 7), digits.slice(7)].filter(Boolean).join('-');
});
byId('pair').addEventListener('click', () => action(async () => {
  const relay = relayAddress();
  const code = byId('code').value.trim();
  if (!code) throw new Error(t('needCode'));
  forget(); wantPair = false;
  light('joining', t('sPairing')); wait('waitPairTitle', 'waitPairDetail', false);
  try { await api('join', { code, relay }); } catch (cause) { wantPair = true; throw cause; }
  byId('code').value = '';
  await api('connect', {});
}));
for (const id of ['code', 'relay']) byId(id).addEventListener('keydown', (event) => { if (event.key === 'Enter') byId('pair').click(); });
byId('relay-change').addEventListener('click', () => {
  relayChosen = true; byId('relay-preset').hidden = true; byId('relay-row').hidden = false; byId('relay').focus();
});
byId('fix').addEventListener('click', () => { if (fix) action(fix); });
byId('cancel').addEventListener('click', () => action(() => api('disconnect', {})));
byId('disconnect').addEventListener('click', () => action(() => api('disconnect', {})));
byId('repair').addEventListener('click', () => { byId('more').open = false; action(newCode); });
byId('key').addEventListener('click', () => { if (apiKey) byId('key').type = byId('key').type === 'password' ? 'text' : 'password'; });
for (const button of document.querySelectorAll('[data-copy]')) {
  button.addEventListener('click', () => action(async () => {
    const value = { base: byId('base').value, key: apiKey, model: byId('model').hidden ? byId('model-one').value : byId('model').value }[button.dataset.copy];
    if (!value) return;
    try { await navigator.clipboard.writeText(value); }
    catch { const field = byId({ base: 'base', key: 'key', model: 'model-one' }[button.dataset.copy]); field.type = 'text'; field.select(); document.execCommand('copy'); if (field === byId('key')) field.type = 'password'; }
    button.textContent = t('copied'); setTimeout(() => { button.textContent = t('copy'); }, 1500);
  }));
}
byId('config').addEventListener('change', () => action(async () => {
  const file = byId('config').files[0];
  if (!file) return;
  if (file.size > 60000) throw new Error(t('fileTooLarge'));
  await api('config', { toml: await file.text() });
  byId('config').value = ''; forget(); wantPair = false; byId('more').open = false;
}));
byId('update-auto').addEventListener('change', () => action(() => api('update', { action: byId('update-auto').checked ? 'on' : 'off' })));
byId('update-now').addEventListener('click', () => {
  const button = byId('update-now'), open = typeof unsavedChats === 'function' ? unsavedChats() : 0;
  if (open && !button.dataset.sure) {
    button.dataset.sure = '1'; button.textContent = t('updateSure').replace('{n}', open);
    setTimeout(() => { delete button.dataset.sure; button.textContent = t('updateNow'); }, 4000);
    return;
  }
  action(async () => {
    await api('update', { action: 'install' }); closed = true; sessionStorage.removeItem('link-control-token'); forget();
    light('connecting', t('sUpdating')); byId('problem-title').textContent = t('updatingTitle');
    byId('problem-detail').textContent = t('updatingDetail'); byId('fix').hidden = true; show('problem');
    byId('update-note').hidden = true;
    for (const element of document.querySelectorAll('button')) element.disabled = true;
  });
});
byId('quit').addEventListener('click', () => action(async () => {
  await api('quit', {}); closed = true; sessionStorage.removeItem('link-control-token'); forget();
  light('', t('sClosed')); byId('problem-title').textContent = t('sClosed');
  byId('problem-detail').textContent = t('closedDetail'); byId('fix').hidden = true; show('problem');
  for (const button of document.querySelectorAll('button')) button.disabled = true;
}));
async function poll() { await refresh(); if (!closed) setTimeout(poll, 2000); }
poll();
