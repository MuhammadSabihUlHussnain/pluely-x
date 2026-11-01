// Placeholder frontend used only for native spikes/smoke tests (not shipped).
const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args || {});
let n = 0;
document.getElementById('click').addEventListener('click', async () => {
  n += 1;
  document.getElementById('n').textContent = String(n);
  try { await invoke('spike_log', { msg: 'click ' + n }); } catch (e) { document.getElementById('out').textContent = JSON.stringify(e); }
});
document.getElementById('cycle').addEventListener('click', async () => {
  try { await invoke('spike_cycle', { n: 20 }); } catch (e) { document.getElementById('out').textContent = JSON.stringify(e); }
});
