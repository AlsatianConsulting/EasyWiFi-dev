const tabs = [
  { key: 'ap', label: 'Access Points' },
  { key: 'client', label: 'Clients & Probes' },
  { key: 'bt', label: 'Bluetooth' },
];

let activeTab = 'ap';

function fmtTime(v) {
  if (!v) return '—';
  try { return new Date(v).toLocaleTimeString(); } catch { return String(v); }
}

function el(tag, cls, text) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text != null) node.textContent = text;
  return node;
}

function renderTabs() {
  const nav = document.querySelector('.tabs');
  nav.innerHTML = '';
  tabs.forEach(t => {
    const b = el('button', `tab ${activeTab === t.key ? 'active' : ''}`, t.label);
    b.onclick = () => { activeTab = t.key; renderState(window.__state || {}); };
    nav.appendChild(b);
  });
}

function renderTableShell(state) {
  const shell = document.querySelector('.table-shell');
  shell.innerHTML = '';

  const table = el('table', 'ew-table');
  table.style.width = '100%';
  table.style.borderCollapse = 'collapse';
  table.style.fontSize = '12px';

  const thead = el('thead');
  const tbody = el('tbody');

  const defs = {
    ap: {
      cols: ['SSID', 'BSSID', 'OUI', 'CH', 'Enc', 'RSSI', 'Clients', 'First', 'Last', 'HS'],
      rows: (state.access_points || []).map(a => [
        a.ssid || 'Hidden', a.bssid, a.oui_manufacturer || 'Unknown', a.channel ?? '—',
        a.encryption_short, a.rssi_dbm ?? '—', a.number_of_clients ?? 0,
        fmtTime(a.first_seen), fmtTime(a.last_seen), a.handshake_count ?? 0,
      ]),
    },
    client: {
      cols: ['MAC', 'OUI', 'AP', 'RSSI', 'Probes', 'First', 'Last', 'Data'],
      rows: (state.clients || []).map(c => [
        c.mac, c.oui_manufacturer || 'Unknown', c.associated_ap || '—', c.rssi_dbm ?? '—',
        (c.probes || []).join(', ') || '—', fmtTime(c.first_seen), fmtTime(c.last_seen), c.data_transferred_bytes ?? 0,
      ]),
    },
    bt: {
      cols: ['Name', 'MAC', 'OUI', 'RSSI', 'Mfgr IDs', 'First', 'Last', 'UUIDs'],
      rows: (state.bluetooth_devices || []).map(d => [
        d.advertised_name || d.alias || 'Unknown', d.mac, d.oui_manufacturer || 'Unknown', d.rssi_dbm ?? '—',
        (d.mfgr_ids || []).join(', ') || '—', fmtTime(d.first_seen), fmtTime(d.last_seen),
        (d.uuid_names || d.uuids || []).join(', ') || '—',
      ]),
    },
  };

  const cfg = defs[activeTab];
  const headRow = el('tr');
  cfg.cols.forEach(c => {
    const th = el('th', null, c);
    th.style.textAlign = 'left';
    th.style.padding = '8px';
    th.style.borderBottom = '1px solid hsl(240,6%,18%)';
    th.style.color = 'hsl(215,15%,55%)';
    headRow.appendChild(th);
  });
  thead.appendChild(headRow);

  cfg.rows.slice(0, 400).forEach(r => {
    const tr = el('tr');
    r.forEach(v => {
      const td = el('td', null, String(v));
      td.style.padding = '8px';
      td.style.borderBottom = '1px solid hsla(240,6%,18%,0.5)';
      td.style.whiteSpace = 'nowrap';
      tr.appendChild(td);
    });
    tbody.appendChild(tr);
  });

  table.appendChild(thead);
  table.appendChild(tbody);
  shell.appendChild(table);
}

function renderState(state) {
  window.__state = state;
  document.getElementById('aps-count').textContent = (state.access_points || []).length;
  document.getElementById('clients-count').textContent = (state.clients || []).length;
  document.getElementById('scan-state').textContent = state.scanning_wifi || state.scanning_bluetooth ? 'Scanning' : 'Idle';
  const h = document.getElementById('health');
  h.textContent = `Wi-Fi: ${state.scanning_wifi ? 'on' : 'off'} | Bluetooth: ${state.scanning_bluetooth ? 'on' : 'off'} | Logs: ${(state.logs || []).length}`;
  renderTabs();
  renderTableShell(state);

  const btn = document.getElementById('scan-btn');
  const scanning = state.scanning_wifi || state.scanning_bluetooth;
  btn.textContent = scanning ? 'Stop Scan' : 'Start Scan';
  btn.className = `btn ${scanning ? '' : 'primary'}`;
}

async function refresh() {
  try {
    const res = await fetch('/api/state');
    const state = await res.json();
    renderState(state);
  } catch (err) {
    document.getElementById('health').textContent = `Backend unavailable: ${err}`;
  }
}

async function post(path) {
  await fetch(path, { method: 'POST' });
  await refresh();
}

document.getElementById('scan-btn').addEventListener('click', async () => {
  const s = window.__state || {};
  const scanning = s.scanning_wifi || s.scanning_bluetooth;
  await post(scanning ? '/api/scan/stop' : '/api/scan/start');
});

document.getElementById('settings-btn').addEventListener('click', () => {
  alert('Settings UI migration in progress. Existing settings are preserved in backend.');
});

refresh();
setInterval(refresh, 1200);
