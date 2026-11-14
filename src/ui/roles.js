'use strict';
// A playground for role profiles: several roles answer in one conversation, each with its
// model, its task and the tools it may use. Nothing is run: a tool call shows what the role
// would do, and the user decides what it gets back. The conversation stays in this page.

const ROLE_STEPS = 8;
const roleToolNames = ['archive_search', 'email_read', 'image_create', 'video_create', 'hand_over'];
const roleToolLabels = {
  archive_search: 'toolArchive', email_read: 'toolEmail', image_create: 'toolImage',
  video_create: 'toolVideo', hand_over: 'toolHandOver'
};
function roleToolSchema(name, others) {
  const text = (description) => ({ type: 'string', description });
  const tools = {
    archive_search: ['Search the archive. Returns matching excerpts, never whole documents or lists.',
      { query: text('What to look for') }, ['query']],
    email_read: ['Read e-mails in the archive that match a search. Returns the relevant passages.',
      { query: text('Sender, subject or topic') }, ['query']],
    image_create: ['Create an image from a description.', { prompt: text('What the image shows') }, ['prompt']],
    video_create: ['Create a short video from a description, optionally starting from an image.',
      { prompt: text('What happens in the video'), image: text('Which earlier image to start from, if any') }, ['prompt']],
    hand_over: ['Hand the work to another role, with a clear task.',
      { role: { type: 'string', enum: others }, task: text('What the other role should do') }, ['role', 'task']]
  };
  const [description, properties, required] = tools[name];
  return { type: 'function', function: { name, description, parameters: { type: 'object', properties, required } } };
}

// A project: its rule sets (what the group agreed, e.g. for code or for design) and its
// roles. Each role names the rule sets that apply to it; it gets them before its own task.
const roleState = { project: '', ruleSets: [], roles: [], selected: null, entries: [], busy: null, steps: 0, examples: false };
let roleIds = 0;
const newRoleId = () => 'r' + (++roleIds);

function exampleProject() {
  const de = language === 'de';
  const code = { id: newRoleId(), name: 'Code', rules: de
    ? 'C# und Rust. KISS, minimalistisch, lesbarer Code. Höchstens 2.000 Zeilen je Datei. Dokumentation im Code, eine README.md je Bereich.'
    : 'C# and Rust. KISS, minimal, readable code. At most 2,000 lines per file. Documentation in the code, a README.md per area.' };
  const design = { id: newRoleId(), name: de ? 'Gestaltung' : 'Design', rules: de
    ? 'Klare, kurze Texte. Barrierefrei (Kontrast, Tastatur, Bildschirmleser). Hell und dunkel. Funktioniert ab Handybreite.'
    : 'Clear, short texts. Accessible (contrast, keyboard, screen readers). Light and dark. Works from phone width up.' };
  roleState.project = de ? 'Beispielprojekt' : 'Example project';
  roleState.ruleSets = [code, design];
  return [
    { name: 'Designer', ruleSets: [design.id], tools: ['image_create', 'hand_over'], instructions: de
      ? 'Du entwirfst Oberflächen. Beschreibe Entwürfe knapp, erstelle bei Bedarf ein Bild und gib fertige Entwürfe an den Reviewer.'
      : 'You design user interfaces. Describe designs briefly, create an image when useful and hand finished designs to the Reviewer.' },
    { name: 'Coder', ruleSets: [code.id], tools: ['hand_over'], instructions: de
      ? 'Du setzt Entwürfe in Code um und gibst fertigen Code an den Reviewer.'
      : 'You turn designs into code and hand finished code to the Reviewer.' },
    { name: 'Reviewer', ruleSets: [code.id, design.id], tools: ['hand_over'], instructions: de
      ? 'Du prüfst Entwürfe auf Verständlichkeit, Barrierefreiheit und Fehler. Nenne konkrete Mängel und gib bei Bedarf an den Designer zurück.'
      : 'You review designs for clarity, accessibility and mistakes. Name concrete problems and hand back to the Designer when needed.' },
    { name: de ? 'Archivar' : 'Archivist', ruleSets: [], tools: ['archive_search', 'email_read'], instructions: de
      ? 'Du beantwortest Fragen aus dem Archiv und den E-Mails darin. Gib nur die Information weiter, nach der gefragt wurde. Nenne nie Listen, Anzahlen oder ganze Dokumente aus dem Archiv.'
      : 'You answer questions from the archive and the e-mails in it. Pass on only the information asked for. Never reveal lists, counts or whole documents from the archive.' }
  ].map((role) => ({ ...role, id: newRoleId(), model: '' }));
}

const roleById = (id) => roleState.roles.find((role) => role.id === id);
const roleName = (id) => roleById(id)?.name || t('roleGone');

/* ---- the role list and editor ---- */
function chatModelIds() {
  const capability = new Map(modelCatalog.map((model) => [model.id, model.capability]));
  return (models || []).filter((id) => !capability.get(id) || capability.get(id) === 'chat');
}
function drawRoles() {
  byId('role-list').replaceChildren(...roleState.roles.map((role) => {
    const item = el('li');
    const button = el('button', 'chat-list-title', role.name || t('roleUnnamed')); button.type = 'button';
    button.setAttribute('aria-current', String(role.id === roleState.selected));
    button.addEventListener('click', () => { roleState.selected = role.id; drawRoles(); });
    item.append(button);
    return item;
  }));
  const role = roleById(roleState.selected);
  byId('role-form').hidden = !role;
  if (role) {
    if (document.activeElement !== byId('role-name')) byId('role-name').value = role.name;
    if (document.activeElement !== byId('role-instructions')) byId('role-instructions').value = role.instructions;
    const ids = chatModelIds();
    fillChoices(byId('role-model'), [{ value: '', label: t('chatDefaultModel') }, ...ids.map((id) => ({ value: id, label: modelLabel(id) }))]);
    byId('role-model').value = ids.includes(role.model) ? role.model : '';
    byId('role-tools').replaceChildren(...roleToolNames.map((name) => {
      const label = el('label', 'check');
      const box = el('input'); box.type = 'checkbox'; box.checked = role.tools.includes(name);
      box.addEventListener('change', () => {
        role.tools = roleToolNames.filter((tool) => tool === name ? box.checked : role.tools.includes(tool));
      });
      label.append(box, el('span', '', t(roleToolLabels[name])));
      return label;
    }));
    byId('role-rule-sets').replaceChildren(...roleState.ruleSets.map((set) => {
      const label = el('label', 'check');
      const box = el('input'); box.type = 'checkbox'; box.checked = role.ruleSets.includes(set.id);
      box.addEventListener('change', () => {
        role.ruleSets = roleState.ruleSets.map((item) => item.id)
          .filter((id) => id === set.id ? box.checked : role.ruleSets.includes(id));
      });
      label.append(box, el('span', '', set.name || t('ruleSetUnnamed')));
      return label;
    }));
    byId('role-rule-sets').closest('fieldset').hidden = !roleState.ruleSets.length;
  }
  const speaker = byId('roles-speaker'), previous = speaker.value;
  fillChoices(speaker, roleState.roles.map((item) => ({ value: item.id, label: item.name || t('roleUnnamed') })));
  if (roleById(previous)) speaker.value = previous;
}
byId('role-name').addEventListener('input', () => {
  const role = roleById(roleState.selected);
  if (role) { role.name = byId('role-name').value.replace(/\s+/g, ' ').slice(0, 40); drawRoles(); }
});
byId('role-instructions').addEventListener('input', () => {
  const role = roleById(roleState.selected);
  if (role) role.instructions = byId('role-instructions').value.slice(0, 8000);
});
byId('role-model').addEventListener('change', () => {
  const role = roleById(roleState.selected);
  if (role) role.model = byId('role-model').value;
});
byId('role-model').addEventListener('focus', freshModels);
byId('roles-project').addEventListener('input', () => { roleState.project = byId('roles-project').value.slice(0, 60); });
// The project's rule sets: a name and the rules, each editable in place.
function drawRuleSets() {
  byId('roles-project').value = roleState.project;
  byId('rule-sets').replaceChildren(...roleState.ruleSets.map((set) => {
    const box = el('div', 'rule-set');
    const name = el('input'); name.type = 'text'; name.maxLength = 40; name.value = set.name;
    name.placeholder = t('ruleSetUnnamed'); name.setAttribute('aria-label', t('roleName'));
    name.addEventListener('input', () => { set.name = name.value.replace(/\s+/g, ' '); drawRoles(); });
    const rules = el('textarea'); rules.rows = 3; rules.value = set.rules; rules.placeholder = t('rolesRulesHint');
    rules.setAttribute('aria-label', t('rolesRuleSets'));
    rules.addEventListener('input', () => { set.rules = rules.value.slice(0, 8000); });
    const remove = el('button', 'link', t('studioRemove')); remove.type = 'button';
    remove.addEventListener('click', () => confirmed(remove, t('chatSure'), () => {
      roleState.ruleSets = roleState.ruleSets.filter((item) => item !== set);
      for (const role of roleState.roles) role.ruleSets = role.ruleSets.filter((id) => id !== set.id);
      drawRuleSets(); drawRoles();
    }));
    box.append(name, rules, remove);
    return box;
  }));
}
byId('ruleset-new').addEventListener('click', () => {
  roleState.ruleSets.push({ id: newRoleId(), name: '', rules: '' });
  drawRuleSets(); drawRoles();
  byId('rule-sets').lastElementChild.querySelector('input').focus();
});
byId('role-new').addEventListener('click', () => {
  const role = { id: newRoleId(), name: t('roleUnnamed'), model: '', instructions: '', tools: [], ruleSets: [] };
  roleState.roles.push(role);
  roleState.selected = role.id;
  drawRoles();
  byId('role-name').focus(); byId('role-name').select();
});
byId('role-delete').addEventListener('click', () => confirmed(byId('role-delete'), t('chatSure'), () => {
  roleState.roles = roleState.roles.filter((role) => role.id !== roleState.selected);
  roleState.selected = roleState.roles[0]?.id || null;
  drawRoles();
}));
byId('roles-export').addEventListener('click', () => {
  const setName = (id) => roleState.ruleSets.find((set) => set.id === id)?.name;
  const data = {
    version: 2, project: roleState.project,
    rule_sets: roleState.ruleSets.map(({ name, rules }) => ({ name, rules })),
    roles: roleState.roles.map(({ name, model, instructions, tools, ruleSets }) =>
      ({ name, model, instructions, tools, rule_sets: ruleSets.map(setName).filter(Boolean) }))
  };
  const link = el('a');
  link.href = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' }));
  link.download = 'k-lani-link-roles.json';
  link.click();
  setTimeout(() => URL.revokeObjectURL(link.href), 1000);
});
byId('roles-import').addEventListener('click', () => byId('roles-file').click());
byId('roles-file').addEventListener('change', async () => {
  const file = byId('roles-file').files[0];
  byId('roles-file').value = '';
  if (!file) return;
  try {
    if (file.size > 256 * 1024) throw new Error();
    const data = JSON.parse(await file.text());
    if (!Array.isArray(data.roles)) throw new Error();
    const sets = (Array.isArray(data.rule_sets) ? data.rule_sets : []).slice(0, 16).map((set) => ({
      id: newRoleId(), name: String(set.name || '').replace(/\s+/g, ' ').slice(0, 40), rules: String(set.rules || '').slice(0, 8000)
    }));
    const byName = (names) => Array.isArray(names) ? sets.filter((set) => names.includes(set.name)).map((set) => set.id) : [];
    const roles = data.roles.slice(0, 12).map((role) => ({
      id: newRoleId(),
      name: String(role.name || '').replace(/\s+/g, ' ').slice(0, 40) || t('roleUnnamed'),
      model: String(role.model || '').slice(0, 256),
      instructions: String(role.instructions || '').slice(0, 8000),
      tools: Array.isArray(role.tools) ? roleToolNames.filter((name) => role.tools.includes(name)) : [],
      ruleSets: byName(role.rule_sets)
    }));
    if (!roles.length) throw new Error();
    roleState.roles = roles;
    roleState.ruleSets = sets;
    roleState.project = String(data.project || '').slice(0, 60);
    roleState.selected = roles[0].id;
    rolesSay('');
    drawRuleSets();
    drawRoles();
  } catch {
    rolesSay('rolesImportFailed');
  }
});
function rolesSay(key) {
  byId('roles-problem').textContent = key ? t(key) : '';
  byId('roles-problem').hidden = !key;
}

/* ---- the conversation ---- */
// What one role sees: its task, the other roles, and the conversation from its side.
function roleMessages(role) {
  const others = roleState.roles.filter((item) => item.id !== role.id).map((item) => item.name);
  const system = [
    language === 'de'
      ? `Du bist die Rolle „${role.name}“ in einem Gespräch mit mehreren Rollen und einem Menschen.`
      : `You are the role "${role.name}" in a conversation with several roles and a person.`,
    others.length ? (language === 'de' ? 'Andere Rollen: ' : 'Other roles: ') + others.join(', ') + '.' : '',
    roleState.project.trim() ? (language === 'de' ? 'Projekt: ' : 'Project: ') + roleState.project.trim() : '',
    ...roleState.ruleSets.filter((set) => role.ruleSets.includes(set.id) && set.rules.trim())
      .map((set) => (language === 'de' ? `Regeln „${set.name}“, verbindlich:\n` : `Rules "${set.name}", binding:\n`) + set.rules.trim()),
    role.instructions
  ].filter(Boolean).join('\n\n');
  const messages = [{ role: 'system', content: system }];
  const push = (message) => {
    const last = messages.at(-1);
    if (message.role === 'user' && last.role === 'user' && typeof last.content === 'string') last.content += '\n\n' + message.content;
    else messages.push(message);
  };
  for (const entry of roleState.entries) {
    if (entry.kind === 'user') push({ role: 'user', content: entry.content });
    else if (entry.kind === 'say' && entry.content) {
      push(entry.role === role.id ? { role: 'assistant', content: entry.content }
        : { role: 'user', content: `[${roleName(entry.role)}]: ${entry.content}` });
    } else if (entry.kind === 'tool' && entry.result !== undefined) {
      if (entry.role === role.id) {
        messages.push({ role: 'assistant', content: null, tool_calls: [{ id: entry.id, type: 'function',
          function: { name: entry.name, arguments: entry.args } }] });
        messages.push({ role: 'tool', tool_call_id: entry.id, content: entry.result });
      } else {
        push({ role: 'user', content: `[${roleName(entry.role)} → ${entry.name}(${entry.args})]: ${entry.result}` });
      }
    }
  }
  if (messages.at(-1).role === 'assistant') push({ role: 'user', content: language === 'de' ? 'Fahre fort.' : 'Continue.' });
  return messages;
}

async function roleTurn(roleId) {
  const role = roleById(roleId);
  if (!role || roleState.busy) return;
  if (++roleState.steps > ROLE_STEPS) { rolesSay('rolesTooManySteps'); return; }
  const others = roleState.roles.filter((item) => item.id !== role.id).map((item) => item.name);
  const tools = role.tools.filter((name) => name !== 'hand_over' || others.length).map((name) => roleToolSchema(name, others));
  const request = { messages: roleMessages(role) };
  if (role.model) request.model = role.model;
  if (tools.length) request.tools = tools;
  const entry = { kind: 'say', role: role.id, content: '', reasoning: '', model: role.model, live: true };
  roleState.entries.push(entry);
  const controller = new AbortController();
  roleState.busy = controller;
  rolesDraw();
  const calls = [];
  try {
    const response = await fetch('/api/chat', {
      method: 'POST', cache: 'no-store', signal: controller.signal,
      headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' }, body: JSON.stringify(request)
    });
    if (!response.ok) throw new Error(said(await response.json().catch(() => ({}))));
    const reader = response.body.getReader(), decoder = new TextDecoder();
    let buffer = '', queued = false;
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let end;
      while ((end = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, end).trim(); buffer = buffer.slice(end + 1);
        if (!line.startsWith('data:') || line.slice(5).trim() === '[DONE]') continue;
        let chunk;
        try { chunk = JSON.parse(line.slice(5)); } catch { continue; }
        if (chunk.error) throw new Error(said(chunk));
        const delta = chunk.choices?.[0]?.delta || {};
        entry.content += delta.content || '';
        entry.reasoning += delta.reasoning_content || delta.reasoning || '';
        for (const part of delta.tool_calls || []) {
          const call = calls[part.index ?? calls.length] ||= { id: '', name: '', args: '' };
          if (part.id) call.id = part.id;
          if (part.function?.name) call.name += part.function.name;
          if (part.function?.arguments) call.args += part.function.arguments;
        }
        if (!queued) { queued = true; requestAnimationFrame(() => { queued = false; rolesDraw(); }); }
      }
    }
  } catch (cause) {
    entry.error = cause.name === 'AbortError' ? t('chatStopped') : cause instanceof TypeError ? t('chatCut') : cause.message;
  } finally {
    entry.content = split(entry).content;
    entry.live = false;
    roleState.busy = null;
  }
  if (!entry.content && !entry.error) roleState.entries.splice(roleState.entries.indexOf(entry), 1);
  for (const call of calls.filter((item) => roleToolNames.includes(item.name))) {
    roleState.entries.push({ kind: 'tool', role: role.id, id: call.id || 'call_' + chatId().slice(0, 12), name: call.name, args: call.args || '{}' });
  }
  rolesDraw();
}

// Once every tool call of a role has its answer, that role goes on.
function settle(entry, result, next) {
  entry.result = result;
  const open = roleState.entries.some((item) => item.kind === 'tool' && item.role === entry.role && item.result === undefined);
  rolesDraw();
  if (!open) roleTurn(next || entry.role);
}

function toolArgs(entry) {
  try { return JSON.parse(entry.args); } catch { return {}; }
}

function toolCard(entry) {
  const box = el('div', 'role-tool');
  const args = toolArgs(entry);
  const head = el('p', 'role-tool-head');
  head.append(el('span', 'role-badge', roleName(entry.role)), ' ',
    t(entry.name === 'hand_over' ? 'rolesWantsHandOver' : 'rolesWantsTool').replace('{tool}', t(roleToolLabels[entry.name]))
      .replace('{role}', String(args.role || '?')));
  box.append(head);
  const shown = el('pre', 'role-tool-args'); shown.textContent = JSON.stringify(args, null, 2);
  box.append(shown);
  if (entry.result !== undefined) {
    box.append(el('p', 'role-tool-result', (language === 'de' ? 'Simuliertes Ergebnis: ' : 'Simulated result: ') + entry.result));
    return box;
  }
  const actions = el('div', 'studio-actions');
  if (entry.name === 'hand_over') {
    const target = roleState.roles.find((role) => role.name === args.role && role.id !== entry.role);
    const go = el('button', 'secondary small', t('rolesRunHandOver')); go.type = 'button';
    go.disabled = !target || !!roleState.busy;
    go.addEventListener('click', () => settle(entry, (language === 'de' ? 'Übergeben an ' : 'Handed to ') + target.name + '.', target.id));
    actions.append(go);
  } else {
    const result = el('textarea', 'role-tool-input'); result.rows = 3;
    result.placeholder = t('rolesResultHint');
    const give = el('button', 'secondary small', t('rolesGiveResult')); give.type = 'button';
    give.disabled = !!roleState.busy;
    give.addEventListener('click', () => settle(entry, result.value.trim() || t('rolesEmptyResult')));
    box.append(result);
    actions.append(give);
  }
  const refuse = el('button', 'link', t('rolesRefuse')); refuse.type = 'button';
  refuse.disabled = !!roleState.busy;
  refuse.addEventListener('click', () => settle(entry, t('rolesRefused')));
  actions.append(refuse);
  box.append(actions);
  return box;
}

function rolesDraw() {
  const log = byId('roles-log');
  const follow = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  byId('roles-thread').replaceChildren(...roleState.entries.map((entry) => {
    if (entry.kind === 'user') { const box = el('div', 'msg user'); box.append(el('div', 'msg-text', entry.content)); return box; }
    if (entry.kind === 'tool') return toolCard(entry);
    const box = el('div', 'msg assistant role-say');
    const head = el('p', 'role-say-head');
    head.append(el('span', 'role-badge', roleName(entry.role)), el('span', 'role-model', modelLabel(entry.model || '') || t('chatDefaultModel')));
    box.append(head);
    if (entry.reasoning && !entry.content) box.append(el('div', 'msg-progress', t('chatThinking') + ' …'));
    const text = el('div', 'msg-text'); writeText(text, split(entry).content); box.append(text);
    if (entry.live && !entry.content && !entry.reasoning) box.append(el('div', 'msg-progress', t('chatWaiting')));
    if (entry.error) box.append(el('p', 'studio-failed', entry.error));
    return box;
  }));
  byId('roles-empty').hidden = roleState.entries.length > 0;
  if (follow || !roleState.busy) log.scrollTop = log.scrollHeight;
  byId('roles-send').textContent = t(roleState.busy ? 'chatStop' : 'chatSend');
  byId('roles-send').classList.toggle('stop', !!roleState.busy);
}

byId('roles-input').addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    if (!roleState.busy) byId('roles-composer').requestSubmit();
  }
});
byId('roles-composer').addEventListener('submit', (event) => {
  event.preventDefault();
  if (roleState.busy) { roleState.busy.abort(); return; }
  const text = byId('roles-input').value.trim();
  const speaker = byId('roles-speaker').value;
  if (!speaker) { rolesSay('rolesNone'); return; }
  rolesSay('');
  // Open tool calls end without a result when the person writes again.
  for (const entry of roleState.entries) if (entry.kind === 'tool' && entry.result === undefined) entry.result = t('rolesRefused');
  if (text) roleState.entries.push({ kind: 'user', content: text });
  byId('roles-input').value = '';
  roleState.steps = 0;
  roleTurn(speaker);
});
byId('roles-clear').addEventListener('click', () => {
  if (roleState.busy || !roleState.entries.length) return;
  confirmed(byId('roles-clear'), t('chatSure'), () => { roleState.entries = []; rolesSay(''); rolesDraw(); });
});

function rolesModels() {
  byId('open-roles').hidden = !chatModelIds().length;
  if (byId('open-roles').hidden && !byId('roles-workspace').hidden) workspace('chat');
  if (!roleState.examples && chatModelIds().length) {
    roleState.examples = true;
    roleState.roles = exampleProject();
    drawRuleSets();
    roleState.selected = roleState.roles[0].id;
  }
  drawRoles();
}
function rolesRelabel() {
  byId('roles-input').placeholder = t('rolesPlaceholder');
  drawRuleSets();
  if (byId('roles-problem').textContent) rolesSay('');
  drawRoles();
  if (!roleState.busy) rolesDraw();
}
function rolesUnsaved() {
  return roleState.entries.length ? 1 : 0;
}
window.addEventListener('pagehide', () => roleState.busy?.abort());
rolesRelabel();
