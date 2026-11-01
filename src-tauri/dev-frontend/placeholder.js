// Placeholder frontend for native smoke tests (not shipped; the real UI lives in src/).
// The main window runs a command sequence and reports each result via log_write as
// "SMOKE <step> <json>" so an external harness can read it from the log file.
const T = window.__TAURI_INTERNALS__;
const label = T.metadata.currentWindow.label;
const out = document.getElementById('out');
document.getElementById('label').textContent = label + ' ' + location.pathname + location.search;
const invoke = (cmd, args) => T.invoke(cmd, args || {});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const report = async (step, value) => {
  const line = 'SMOKE ' + step + ' ' + JSON.stringify(value);
  out.textContent += line + '\n';
  try { await invoke('log_write', { level: 'info', message: line.slice(0, 8000) }); } catch (e) { out.textContent += 'log_write failed ' + JSON.stringify(e) + '\n'; }
};
const run = async (step, fn) => {
  try { const v = await fn(); await report(step, { ok: true, value: v }); return v; }
  catch (e) { await report(step, { ok: false, error: e }); return undefined; }
};
const listen = (event, cb) => invoke('plugin:event|listen', { event, target: { kind: 'Any' }, handler: T.transformCallback(cb) });
const channel = (onMsg) => '__CHANNEL__:' + T.transformCallback((raw) => { if (raw && 'message' in raw) onMsg(raw.message); });

async function main() {
  for (const ev of ['window://state', 'window://typing', 'shortcut://action', 'shortcut://error', 'settings://changed', 'screenshot://captured', 'screenshot://cancelled', 'keys://changed', 'app://error']) {
    await listen(ev, (e) => {
      const p = e.payload;
      const brief = ev === 'screenshot://captured' ? { id: p.id, w: p.width, h: p.height } : ev === 'settings://changed' ? { opacity: p.overlay.opacity } : p;
      report('EVENT ' + ev, brief);
    });
  }
  const input = document.getElementById('text');
  input.addEventListener('mousedown', () => { run('begin_typing_click', () => invoke('window_begin_typing')); });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { report('typed', input.value); run('end_typing_enter', () => invoke('window_end_typing', { restoreFocus: true })); }
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') run('end_typing_esc', () => invoke('window_end_typing', { restoreFocus: true }));
  });

  await run('app_version', () => invoke('app_version'));
  const s = await run('settings_get', () => invoke('settings_get'));
  await run('keys_status', () => invoke('keys_status'));
  await run('keys_test_anthropic_bogus', () => invoke('keys_test', { provider: 'anthropic', candidate: 'sk-ant-bogus-smoke-0000' }));
  await run('keys_test_deepgram_bogus', () => invoke('keys_test', { provider: 'deepgram', candidate: 'bogus0smoke0deepgram0000' }));
  await run('keys_test_anthropic_missing', () => invoke('keys_test', { provider: 'anthropic' }));
  await run('window_get_state', () => invoke('window_get_state'));
  const st = await run('shortcuts_status', () => invoke('shortcuts_status'));
  if (st && st.failures.length) {
    // keys taken by another app on this machine: move just those to free keys so the hotkey paths can be tested
    const alt = { toggle_overlay: 'ctrl+alt+f11', focus_input: 'ctrl+alt+f12' };
    const patch = {};
    for (const f of st.failures) if (alt[f.action]) patch[f.action] = { key: alt[f.action], enabled: true };
    await run('rebind_failed', async () => (await invoke('settings_update', { patch: { shortcuts: patch } })).shortcuts);
    await run('shortcuts_status_after', () => invoke('shortcuts_status'));
  }
  await run('shortcuts_validate', async () => [await invoke('shortcuts_validate', { key: 'ctrl+alt+k' }), await invoke('shortcuts_validate', { key: 'k' })]);
  const shot = await run('screenshot_full', async () => { const m = await invoke('screenshot_full', {}); return { ...m, thumbDataUrl: m.thumbDataUrl.slice(0, 30) + '…(' + m.thumbDataUrl.length + ')' }; });
  if (shot) {
    await run('screenshot_get', async () => { const d = await invoke('screenshot_get', { id: shot.id }); return { mime: d.mime, b64len: d.dataBase64.length }; });
    const saved = await run('screenshot_save', () => invoke('screenshot_save', { id: shot.id, sessionId: 'smoke' }));
    if (saved) await run('screenshot_load', async () => { const d = await invoke('screenshot_load', { path: saved.path }); return { mime: d.mime, b64len: d.dataBase64.length }; });
    await run('screenshot_load_outside', () => invoke('screenshot_load', { path: 'C:\Windows\win.ini' }));
  }
  await run('llm_stream_no_key', async () => {
    const events = [];
    await invoke('llm_stream', { requestId: 'smoke-1', body: { model: 'claude-opus-5-5', max_tokens: 16, messages: [{ role: 'user', content: 'hi' }] }, onEvent: channel((m) => events.push(m)) });
    await sleep(200);
    return events;
  });
  await run('settings_update_opacity', async () => (await invoke('settings_update', { patch: { overlay: { opacity: 0.9 } } })).overlay.opacity);
  await run('settings_update_invalid', () => invoke('settings_update', { patch: { overlay: { opacity: 5 } } }));
  await run('window_set_size', async () => (await invoke('window_set_size', { width: 460, height: 540 })).bounds);
  await run('window_move_by', async () => (await invoke('window_move_by', { dx: 40, dy: 0 })).bounds);

  // region capture: overlays stay open for 3 s so the harness can inspect them
  if (shot) {
    await run('screenshot_region_start', () => invoke('screenshot_region_start'));
    await report('REGION_OPEN', {});
    await sleep(3000);
    await run('screenshot_region_commit', async () => { const m = await invoke('screenshot_region_commit', { monitor: shot.monitor, rect: { x: 100, y: 100, width: 200, height: 100 } }); return { ...m, thumbDataUrl: '…' }; });
  }
  await run('dashboard_open', () => invoke('dashboard_open', { route: '/dashboard/settings' }));
  await report('DASHBOARD_OPEN', {});
  await sleep(2500);
  await run('dashboard_toggle', () => invoke('dashboard_toggle', {}));
  await run('dashboard_bad_route', () => invoke('dashboard_open', { route: 'https://example.com' }));
  await run('log_write', () => invoke('log_write', { level: 'warn', message: 'smoke warn with sk-ant-should-be-redacted' }));
  await report('SMOKE_DONE', {});
}

if (label === 'main') main();
