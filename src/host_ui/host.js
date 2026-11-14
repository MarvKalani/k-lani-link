'use strict';
// The host setup page. It reads the host's overview and edits the settings file; all text
// from the files and the models is shown as text, never as markup.
const byId = (id) => document.getElementById(id);
const bootstrap = location.hash.slice(1);
if (bootstrap) { sessionStorage.setItem('link-host-token', bootstrap); history.replaceState(null, '', '/'); }
const token = sessionStorage.getItem('link-host-token') || '';
const TOOLS = ['archive_search', 'email_read', 'image_create', 'video_create', 'hand_over'];
const texts = {
  en: {
    title: 'Model computer', close: 'Close page', overview: 'Overview', configFile: 'Configuration', settingsFile: 'Settings file',
    upstream: 'Main backend', backends: 'Further backends', models: 'Models the host offers', noBackends: 'None',
    linkHint: 'The host does not read this settings file yet. Add this line at the top of the configuration, then restart the host:',
    dockerHint: 'In Docker, also mount the settings file into the container at the same path.', copy: 'Copy', copied: 'Copied',
    cardsTitle: 'Model cards', cardsIntro: 'Shown in the control panel next to the model. Links must start with https://. The README is a .md or .txt file on this computer (absolute path, at most 32 KB).',
    page: 'Model page', benchmarks: 'Benchmarks', readme: 'README file',
    projectTitle: 'Project, rule sets and roles',
    projectIntro: 'Roles take effect once the role adapter runs (next step). Clients will then see the role and its description, never its task, rule sets or the model behind it.',
    import: 'Load file from the roles playground', importFailed: 'This file holds no roles.', imported: 'Loaded. Check the models, then save.',
    project: 'Project', ruleSets: 'Rule sets', roles: 'Roles', add: 'Add', remove: 'Remove', name: 'Name', rules: 'Rules',
    description: 'Description for clients', descriptionHint: 'What the role does, as clients see it',
    model: 'Model', defaultModel: 'Host default', instructions: 'Task and rules (stays on this computer)',
    roleRuleSets: 'Rule sets for this role', tools: 'May use (not run yet)',
    archive_search: 'Search the archive', email_read: 'Read e-mails', image_create: 'Create an image', video_create: 'Create a video', hand_over: 'Hand over to another role',
    save: 'Save', saved: 'Saved. Restart the host so that changed model cards apply.', closed: 'The page is closed. You can close this tab.',
    unknownModel: 'not offered by this host', features: { video: 'video', speech: 'speech', transcriptions: 'transcription', image_edits: 'image edits', unverified: 'no privacy check' }
  },
  de: {
    title: 'Modellrechner', close: 'Seite schließen', overview: 'Überblick', configFile: 'Konfiguration', settingsFile: 'Einstellungsdatei',
    upstream: 'Haupt-Backend', backends: 'Weitere Backends', models: 'Modelle, die der Host anbietet', noBackends: 'Keine',
    linkHint: 'Der Host liest diese Einstellungsdatei noch nicht. Füge diese Zeile oben in die Konfiguration ein und starte den Host neu:',
    dockerHint: 'Bei Docker binde die Einstellungsdatei zusätzlich unter demselben Pfad in den Container ein.', copy: 'Kopieren', copied: 'Kopiert',
    cardsTitle: 'Modellkarten', cardsIntro: 'Erscheinen im Kontrollfeld neben dem Modell. Links beginnen mit https://. Die README ist eine .md- oder .txt-Datei auf diesem Rechner (absoluter Pfad, höchstens 32 KB).',
    page: 'Modellseite', benchmarks: 'Benchmarks', readme: 'README-Datei',
    projectTitle: 'Projekt, Regelwerke und Rollen',
    projectIntro: 'Rollen werden wirksam, sobald der Rollen-Adapter läuft (nächster Schritt). Clients sehen dann die Rolle und ihre Beschreibung, nie ihren Auftrag, ihre Regelwerke oder das Modell dahinter.',
    import: 'Datei aus der Rollen-Spielwiese laden', importFailed: 'In dieser Datei stehen keine Rollen.', imported: 'Geladen. Prüfe die Modelle und speichere dann.',
    project: 'Projekt', ruleSets: 'Regelwerke', roles: 'Rollen', add: 'Hinzufügen', remove: 'Entfernen', name: 'Name', rules: 'Regeln',
    description: 'Beschreibung für Clients', descriptionHint: 'Was die Rolle tut, so wie Clients es sehen',
    model: 'Modell', defaultModel: 'Standard des Hosts', instructions: 'Auftrag und Regeln (bleibt auf diesem Rechner)',
    roleRuleSets: 'Regelwerke für diese Rolle', tools: 'Darf benutzen (wird noch nicht ausgeführt)',
    archive_search: 'Archiv durchsuchen', email_read: 'E-Mails lesen', image_create: 'Bild erstellen', video_create: 'Video erstellen', hand_over: 'An andere Rolle übergeben',
    save: 'Speichern', saved: 'Gespeichert. Starte den Host neu, damit geänderte Modellkarten gelten.', closed: 'Die Seite ist geschlossen. Du kannst diesen Tab schließen.',
    unknownModel: 'bietet dieser Host nicht an', features: { video: 'Video', speech: 'Sprache', transcriptions: 'Transkription', image_edits: 'Bildbearbeitung', unverified: 'ohne Datenschutzprüfung' }
  }
};
let language = (navigator.languages || [navigator.language || 'en']).some((tag) => /^de\b/i.test(tag)) ? 'de' : 'en';
const t = (key) => texts[language][key] ?? texts.en[key] ?? key;
let settings = { model_cards: [], project: '', rule_sets: [], roles: [] };
let modelIds = [];

function el(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text !== undefined) element.textContent = text;
  return element;
}
function error(message) { byId('error').textContent = message || ''; byId('error').hidden = !message; }
async function api(path, data) {
  const response = await fetch('/api/' + path, {
    method: data === undefined ? 'GET' : 'POST', cache: 'no-store',
    headers: { Authorization: 'Bearer ' + token, ...(data === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: data === undefined ? undefined : JSON.stringify(data)
  });
  const value = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(value.error || response.statusText);
  return value;
}
function field(labelKey, control) {
  const label = el('label');
  label.append(el('span', '', t(labelKey)), control);
  return label;
}
function input(value, onInput, attributes = {}) {
  const box = el(attributes.rows ? 'textarea' : 'input');
  if (!attributes.rows) box.type = 'text';
  Object.assign(box, attributes);
  box.value = value || '';
  box.addEventListener('input', () => onInput(box.value));
  return box;
}
function removeButton(onRemove) {
  const button = el('button', 'link', t('remove')); button.type = 'button';
  button.addEventListener('click', onRemove);
  return button;
}

/* ---- overview ---- */
async function loadOverview() {
  const view = await api('overview');
  byId('config-file').textContent = view.config;
  byId('settings-file').textContent = view.settings;
  byId('upstream').textContent = view.upstream + (view.unverified ? ' · ' + t('features').unverified : '');
  byId('link-hint').hidden = view.linked;
  byId('link-line').value = view.line;
  byId('backends').replaceChildren(...(view.backends.length ? view.backends.map((backend) => {
    const tags = Object.keys(t('features')).filter((key) => backend[key]).map((key) => t('features')[key]);
    return el('li', '', backend.upstream + (tags.length ? ' · ' + tags.join(', ') : ''));
  }) : [el('li', 'muted', t('noBackends'))]));
  const models = Array.isArray(view.models) ? view.models : [];
  modelIds = models.map((model) => String(model.id));
  byId('models').replaceChildren(...models.map((model) =>
    el('li', '', model.id + (model.capability ? ' · ' + model.capability : '') + (model.owned_by ? ' · ' + model.owned_by : ''))));
  byId('models-error').textContent = view.models_error || '';
  byId('models-error').hidden = !view.models_error;
}

/* ---- model cards: one row per offered model, plus cards for models not offered now ---- */
function drawCards() {
  const ids = [...new Set([...modelIds, ...settings.model_cards.map((card) => card.id)])];
  byId('cards').replaceChildren(...ids.map((id) => {
    let card = settings.model_cards.find((item) => item.id === id);
    const ensure = () => {
      if (!card) { card = { id, page: '', benchmarks: '', readme: '' }; settings.model_cards.push(card); }
      return card;
    };
    const box = el('div', 'host-item');
    box.append(el('h4', '', id + (modelIds.includes(id) ? '' : ' · ' + t('unknownModel'))));
    for (const key of ['page', 'benchmarks', 'readme']) {
      box.append(field(key, input(card?.[key], (value) => { ensure()[key] = value.trim(); },
        { placeholder: key === 'readme' ? '/models/…/README.md' : 'https://', spellcheck: false })));
    }
    return box;
  }));
}

/* ---- project, rule sets, roles ---- */
function drawProject() {
  byId('project').value = settings.project || '';
  byId('rule-sets').replaceChildren(...settings.rule_sets.map((set, index) => {
    const box = el('div', 'host-item');
    let previous = set.name;
    box.append(field('name', input(set.name, (value) => {
      // A renamed rule set stays chosen by the roles that use it.
      for (const role of settings.roles) role.rule_sets = role.rule_sets.map((name) => name === previous ? value : name);
      set.name = value; previous = value;
    }, { maxLength: 40 })));
    // The roles' rule set choices show the new name once the field is left.
    box.querySelector('input').addEventListener('change', drawProject);
    box.append(field('rules', input(set.rules, (value) => { set.rules = value; }, { rows: 4, maxLength: 8000 })));
    box.append(removeButton(() => {
      for (const role of settings.roles) role.rule_sets = role.rule_sets.filter((name) => name !== set.name);
      settings.rule_sets.splice(index, 1); drawProject();
    }));
    return box;
  }));
  byId('roles').replaceChildren(...settings.roles.map((role, index) => {
    const box = el('div', 'host-item');
    box.append(field('name', input(role.name, (value) => { role.name = value; }, { maxLength: 40 })));
    box.append(field('description', input(role.description, (value) => { role.description = value; },
      { maxLength: 500, placeholder: t('descriptionHint') })));
    const model = el('select');
    const ids = [...new Set(['', ...modelIds, role.model || ''])];
    model.replaceChildren(...ids.map((id) => new Option(id ? id + (modelIds.includes(id) ? '' : ' · ' + t('unknownModel')) : t('defaultModel'), id)));
    model.value = role.model || '';
    model.addEventListener('change', () => { role.model = model.value; });
    box.append(field('model', model));
    box.append(field('instructions', input(role.instructions, (value) => { role.instructions = value; }, { rows: 5, maxLength: 8000 })));
    for (const [key, options, chosen] of [['roleRuleSets', settings.rule_sets.map((set) => set.name), 'rule_sets'], ['tools', TOOLS, 'tools']]) {
      if (!options.length) continue;
      const group = el('fieldset', 'host-checks');
      group.append(el('legend', '', t(key)));
      for (const option of options) {
        const label = el('label', 'check');
        const box2 = el('input'); box2.type = 'checkbox'; box2.checked = role[chosen].includes(option);
        box2.addEventListener('change', () => {
          role[chosen] = options.filter((item) => item === option ? box2.checked : role[chosen].includes(item));
        });
        label.append(box2, el('span', '', chosen === 'tools' ? t(option) : option || '…'));
        group.append(label);
      }
      box.append(group);
    }
    box.append(removeButton(() => { settings.roles.splice(index, 1); drawProject(); }));
    return box;
  }));
}
byId('project').addEventListener('input', () => { settings.project = byId('project').value; });
byId('ruleset-add').addEventListener('click', () => { settings.rule_sets.push({ name: '', rules: '' }); drawProject(); });
byId('role-add').addEventListener('click', () => {
  settings.roles.push({ name: '', description: '', model: '', instructions: '', rule_sets: [], tools: [] }); drawProject();
});

// A file from the control panel's roles playground: the same fields, model IDs without the
// panel's "@n" computer suffix.
byId('import').addEventListener('click', () => byId('import-file').click());
byId('import-file').addEventListener('change', async () => {
  const file = byId('import-file').files[0];
  byId('import-file').value = '';
  if (!file) return;
  try {
    if (file.size > 256 * 1024) throw new Error();
    const data = JSON.parse(await file.text());
    if (!Array.isArray(data.roles) || !data.roles.length) throw new Error();
    const text = (value, max) => String(value ?? '').slice(0, max);
    settings.project = text(data.project, 60);
    settings.rule_sets = (Array.isArray(data.rule_sets) ? data.rule_sets : []).slice(0, 32)
      .map((set) => ({ name: text(set.name, 40), rules: text(set.rules, 8000) }));
    const sets = settings.rule_sets.map((set) => set.name);
    settings.roles = data.roles.slice(0, 32).map((role) => ({
      name: text(role.name, 40), description: text(role.description, 500),
      model: text(role.model, 256).replace(/@\d+$/, ''), instructions: text(role.instructions, 8000),
      rule_sets: Array.isArray(role.rule_sets) ? sets.filter((name) => role.rule_sets.includes(name)) : [],
      tools: Array.isArray(role.tools) ? TOOLS.filter((name) => role.tools.includes(name)) : []
    }));
    error(''); drawProject();
    byId('saved').textContent = t('imported');
  } catch {
    error(t('importFailed'));
  }
});

byId('save').addEventListener('click', async () => {
  byId('saved').textContent = '';
  const cards = settings.model_cards.filter((card) => card.page || card.benchmarks || card.readme);
  try {
    await api('settings', { ...settings, model_cards: cards });
    error(''); byId('saved').textContent = t('saved');
  } catch (cause) {
    error(cause.message);
  }
});
byId('link-copy').addEventListener('click', async () => {
  try { await navigator.clipboard.writeText(byId('link-line').value); } catch { byId('link-line').select(); document.execCommand('copy'); }
  byId('link-copy').textContent = t('copied'); setTimeout(() => { byId('link-copy').textContent = t('copy'); }, 1500);
});
byId('close').addEventListener('click', async () => {
  try { await api('quit', {}); } catch { /* already closed */ }
  sessionStorage.removeItem('link-host-token');
  document.querySelector('main').replaceChildren(el('p', 'muted', t('closed')));
});

function applyTexts() {
  document.documentElement.lang = language;
  for (const element of document.querySelectorAll('[data-t]')) element.textContent = t(element.dataset.t);
  for (const button of document.querySelectorAll('[data-language]')) button.setAttribute('aria-pressed', String(button.dataset.language === language));
  drawCards(); drawProject();
}
for (const button of document.querySelectorAll('[data-language]')) {
  button.addEventListener('click', () => { language = button.dataset.language; applyTexts(); });
}

(async () => {
  applyTexts();
  try {
    await loadOverview();
    settings = { model_cards: [], project: '', rule_sets: [], roles: [], ...(await api('settings')) };
    error('');
  } catch (cause) {
    error(cause.message);
  }
  applyTexts();
})();
