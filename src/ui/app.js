'use strict';
const byId = (id) => document.getElementById(id);
const bootstrap = location.hash.slice(1);
if (bootstrap) { sessionStorage.setItem('link-control-token', bootstrap); history.replaceState(null, '', '/'); }
const token = sessionStorage.getItem('link-control-token') || '';
let closed = false;
let busy = false;
let current = null;
let relayChosen = false;
const phases = {
  not_configured: ['Ready to pair', 'Enter the pairing code from the model computer.'],
  joining: ['Waiting for the other computer…', 'Keep this page open. Pairing finishes as soon as the model computer confirms the code.'],
  denied: ['Access refused by the relay', 'The relay token for this computer expired, was revoked or is limited to other addresses. Ask the operator, or pair again with a new code.'],
  disconnected: ['Disconnected', 'Start the matching remote host, then click Connect.'],
  connecting: ['Connecting…', 'Waiting for an authenticated peer. Check the host, matching fresh code, clocks and permitted network access.'],
  connected: ['Connected · encrypted peer tunnel', 'Pairing is complete. Signaling is disconnected and TURN is disabled. Test backend to check model availability.'],
  recovering: ['Connection interrupted · recovering', 'The existing session is trying to recover. An interrupted inference request is not automatically replayed.'],
  rejected: ['Pairing rejected', 'The peer or signaling response could not be authenticated. Disconnect and obtain matching fresh configuration files.'],
  new_code_required: ['Fresh pairing code required', 'This code is consumed or reserved. Disconnect and obtain a new pair; do not delete pairing state.'],
  stopped: ['Transport stopped', 'Disconnect to clear this run, then check the configuration and local port.']
};
async function api(path, data) {
  const response = await fetch('/api/' + path, {
    method: data === undefined ? 'GET' : 'POST',
    headers: { Authorization: 'Bearer ' + token, ...(data === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: data === undefined ? undefined : JSON.stringify(data), cache: 'no-store'
  });
  const value = await response.json().catch(() => ({ error: 'The local application returned an unexpected response.' }));
  if (!response.ok) throw new Error(value.error || 'The local request failed.');
  return value;
}
function error(message) { byId('error').textContent = message; byId('error').hidden = !message; }
function render(state) {
  current = state;
  let phase = phases[state.phase] || ['Unknown state', 'Restart the control panel to refresh its state.'];
  if (state.phase === 'connected' && state.transport === 'https') phase = ['Connected · encrypted relay tunnel', 'The relay forwards only encrypted data. Test backend to check model availability.'];
  byId('status').textContent = phase[0]; byId('detail').textContent = phase[1];
  byId('light').className = 'light ' + state.phase;
  byId('version').textContent = state.version ? 'v' + state.version : '';
  document.title = phase[0] + ' · k-lani-link';
  byId('connect').disabled = busy || !state.configured || state.running;
  byId('disconnect').disabled = busy || !state.running;
  byId('test').disabled = busy || state.phase !== 'connected';
  byId('show-key').disabled = busy || !state.configured;
  byId('config').disabled = busy || state.running;
  byId('pair').disabled = busy || state.running || state.joining;
  byId('code').disabled = byId('relay').disabled = state.running || state.joining;
  if (state.default_relay && !relayChosen) {
    byId('relay-name').textContent = state.default_relay;
    byId('relay-preset').hidden = false; byId('relay-row').hidden = true;
  }
  byId('quit').disabled = busy;
  if (state.api_base) byId('base').value = state.api_base;
}
async function refresh() {
  if (closed) return;
  try { render(await api('status')); }
  catch {
    byId('status').textContent = 'Control panel unavailable';
    byId('detail').textContent = 'The application may have stopped. Open k-lani-link again to reconnect this page.';
    byId('light').className = 'light offline';
    for (const id of ['connect', 'disconnect', 'test', 'show-key', 'config', 'pair', 'quit']) byId(id).disabled = true;
  }
}
async function action(work) {
  if (busy || closed) return;
  busy = true; error(''); if (current) render(current);
  try { await work(); } catch (cause) { error(cause.message || 'The operation failed.'); }
  finally { busy = false; await refresh(); }
}
function hideKey() { byId('key').value = ''; byId('key').hidden = true; byId('copy-key').hidden = true; byId('hide-key').hidden = true; }
byId('config').addEventListener('change', () => action(async () => {
  const file = byId('config').files[0];
  if (!file) return;
  if (file.size > 60000) throw new Error('This configuration file is too large.');
  await api('config', { toml: await file.text() });
  byId('config').value = ''; hideKey(); byId('result').hidden = true;
  byId('saved').textContent = 'Configuration checked and saved privately. You can now connect.';
}));
byId('relay-change').addEventListener('click', () => {
  relayChosen = true; byId('relay-preset').hidden = true; byId('relay-row').hidden = false; byId('relay').focus();
});
byId('pair').addEventListener('click', () => action(async () => {
  const code = byId('code').value.trim();
  if (!code) throw new Error('Enter the pairing code.');
  byId('saved').textContent = '';
  await api('join', { code, relay: byId('relay-row').hidden ? '' : byId('relay').value.trim() });
  byId('code').value = ''; hideKey(); byId('result').hidden = true;
  byId('saved').textContent = 'Paired. Connecting…';
  await api('connect', {});
}));
byId('code').addEventListener('keydown', (event) => { if (event.key === 'Enter') byId('pair').click(); });
byId('connect').addEventListener('click', () => action(async () => { byId('result').hidden = true; await api('connect', {}); }));
byId('disconnect').addEventListener('click', () => action(async () => { await api('disconnect', {}); hideKey(); byId('result').hidden = true; }));
byId('test').addEventListener('click', () => action(async () => {
  const value = await api('test', {});
  byId('result').textContent = value.models.length
    ? 'Backend reachable. Models: ' + value.models.join(', ') + '. Model-list round trip: ' + value.round_trip_ms.toFixed(1) + ' ms. This includes API and tunnel overhead, not inference time.'
    : 'The backend answered, but no loaded model was listed.';
  byId('result').hidden = false;
}));
byId('show-key').addEventListener('click', () => action(async () => {
  const value = await api('credentials', {}); byId('key').value = value.api_key;
  byId('key').hidden = false; byId('copy-key').hidden = false; byId('hide-key').hidden = false;
}));
byId('copy-key').addEventListener('click', () => action(async () => { await navigator.clipboard.writeText(byId('key').value); }));
byId('hide-key').addEventListener('click', hideKey);
byId('quit').addEventListener('click', () => action(async () => {
  await api('quit', {}); closed = true; sessionStorage.removeItem('link-control-token'); hideKey();
  byId('status').textContent = 'Application closed'; byId('detail').textContent = 'The transport is stopping. You can close this tab.';
  byId('light').className = 'light';
  for (const id of ['connect', 'disconnect', 'test', 'show-key', 'config', 'pair', 'quit']) byId(id).disabled = true;
}));
async function poll() { await refresh(); if (!closed) setTimeout(poll, 2000); }
poll();
