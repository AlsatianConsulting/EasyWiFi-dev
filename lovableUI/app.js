const tabs = [
  { key: 'ap', label: 'Access Points' },
  { key: 'client', label: 'Clients & Probes' },
  { key: 'bt', label: 'Bluetooth' },
];

const defaultColumns = {
  ap: ['ssid', 'bssid', 'oui', 'channel', 'encryption', 'rssi', 'clients', 'first_seen', 'last_seen', 'handshakes'],
  client: ['mac', 'oui', 'associated_ap', 'rssi', 'probes', 'first_seen', 'last_seen', 'data'],
  bt: ['name', 'mac', 'oui', 'rssi', 'mfgr_ids', 'first_seen', 'last_seen', 'uuids'],
};

const labels = {
  ap: {
    ssid: 'SSID', bssid: 'BSSID', oui: 'OUI', channel: 'CH', encryption: 'Encryption', rssi: 'RSSI',
    clients: 'Clients', first_seen: 'First Seen', last_seen: 'Last Seen', handshakes: 'Handshakes',
  },
  client: {
    mac: 'MAC', oui: 'OUI', associated_ap: 'Associated AP', rssi: 'RSSI', probes: 'Probes', first_seen: 'First Seen',
    last_seen: 'Last Seen', data: 'Data',
  },
  bt: {
    name: 'Name', mac: 'MAC', oui: 'OUI', rssi: 'RSSI', mfgr_ids: 'Mfgr IDs', first_seen: 'First Seen',
    last_seen: 'Last Seen', uuids: 'UUIDs',
  },
};

let activeTab = 'ap';
let sortState = { column: null, dir: null };
let selectedKey = null;
let state = {};
const filters = { ap: {}, client: {}, bt: {} };

function getColumns(tab) {
  const key = `easywifi.columns.${tab}`;
  const stored = localStorage.getItem(key);
  if (!stored) return [...defaultColumns[tab]];
  try {
    const parsed = JSON.parse(stored);
    if (Array.isArray(parsed) && parsed.length) return parsed;
  } catch {}
  return [...defaultColumns[tab]];
}

function setColumns(tab, cols) {
  localStorage.setItem(`easywifi.columns.${tab}`, JSON.stringify(cols));
}

function fmtTime(v) {
  if (!v) return '—';
  try { return new Date(v).toLocaleString(); } catch { return String(v); }
}

function rowKey(tab, row) {
  if (tab === 'ap') return row.bssid;
  if (tab === 'client') return row.mac;
  return row.mac;
}

function packetMix(row) {
  if (!row) return { management: 0, control: 0, data: 0, other: 0 };
  if (row.packet_mix) return row.packet_mix;
  if (row.network_intel?.packet_mix) return row.network_intel.packet_mix;
  return { management: 0, control: 0, data: 0, other: 0 };
}

function drawPacketDonut(mix) {
  const c = document.getElementById('packet-canvas');
  const ctx = c.getContext('2d');
  ctx.clearRect(0, 0, c.width, c.height);
  const total = (mix.management || 0) + (mix.control || 0) + (mix.data || 0) + (mix.other || 0);
  const cx = c.width / 2;
  const cy = c.height / 2;
  const rOuter = 62;
  const rInner = 34;

  if (!total) {
    ctx.fillStyle = 'hsl(215 15% 55%)';
    ctx.font = '12px Ubuntu';
    ctx.fillText('No packet data', cx - 40, cy);
    return;
  }

  const slices = [
    { k: 'management', c: 'hsl(27 76% 53%)' },
    { k: 'control', c: 'hsl(38 92% 50%)' },
    { k: 'data', c: 'hsl(142 71% 45%)' },
    { k: 'other', c: 'hsl(215 15% 55%)' },
  ];

  let a = -Math.PI / 2;
  slices.forEach(s => {
    const v = mix[s.k] || 0;
    if (!v) return;
    const next = a + (v / total) * Math.PI * 2;
    ctx.beginPath();
    ctx.moveTo(cx, cy);
    ctx.arc(cx, cy, rOuter, a, next);
    ctx.closePath();
    ctx.fillStyle = s.c;
    ctx.fill();
    a = next;
  });

  ctx.beginPath();
  ctx.arc(cx, cy, rInner, 0, Math.PI * 2);
  ctx.fillStyle = 'hsl(240 10% 7%)';
  ctx.fill();

  ctx.fillStyle = 'hsl(210 20% 92%)';
  ctx.font = 'bold 13px Ubuntu';
  ctx.fillText(String(total), cx - 10, cy + 4);
}

function valueFor(tab, row, col) {
  if (tab === 'ap') {
    const map = {
      ssid: row.ssid || 'Hidden',
      bssid: row.bssid,
      oui: row.oui_manufacturer || 'Unknown',
      channel: row.channel ?? '—',
      encryption: row.encryption_short || 'Unknown',
      rssi: row.rssi_dbm ?? '—',
      clients: row.number_of_clients ?? 0,
      first_seen: fmtTime(row.first_seen),
      last_seen: fmtTime(row.last_seen),
      handshakes: row.handshake_count ?? 0,
    };
    return map[col] ?? '';
  }
  if (tab === 'client') {
    const map = {
      mac: row.mac,
      oui: row.oui_manufacturer || 'Unknown',
      associated_ap: row.associated_ap || '—',
      rssi: row.rssi_dbm ?? '—',
      probes: (row.probes || []).join(', ') || '—',
      first_seen: fmtTime(row.first_seen),
      last_seen: fmtTime(row.last_seen),
      data: row.data_transferred_bytes ?? 0,
    };
    return map[col] ?? '';
  }
  const map = {
    name: row.advertised_name || row.alias || 'Unknown',
    mac: row.mac,
    oui: row.oui_manufacturer || 'Unknown',
    rssi: row.rssi_dbm ?? '—',
    mfgr_ids: (row.mfgr_ids || []).join(', ') || '—',
    first_seen: fmtTime(row.first_seen),
    last_seen: fmtTime(row.last_seen),
    uuids: (row.uuid_names || row.uuids || []).join(', ') || '—',
  };
  return map[col] ?? '';
}

function rowsFor(tab) {
  if (tab === 'ap') return [...(state.access_points || [])];
  if (tab === 'client') return [...(state.clients || [])];
  return [...(state.bluetooth_devices || [])];
}

function applyFilters(tab, rows, cols) {
  const globalNeedle = (document.getElementById('global-filter').value || '').toLowerCase().trim();
  return rows.filter(r => {
    if (globalNeedle) {
      const all = cols.map(c => String(valueFor(tab, r, c)).toLowerCase()).join(' | ');
      if (!all.includes(globalNeedle)) return false;
    }
    const f = filters[tab] || {};
    for (const c of cols) {
      const needle = (f[c] || '').toLowerCase().trim();
      if (!needle) continue;
      if (!String(valueFor(tab, r, c)).toLowerCase().includes(needle)) return false;
    }
    return true;
  });
}

function applySort(tab, rows) {
  if (!sortState.column || !sortState.dir) return rows;
  const col = sortState.column;
  const dir = sortState.dir === 'asc' ? 1 : -1;
  return rows.sort((a, b) => {
    const av = String(valueFor(tab, a, col));
    const bv = String(valueFor(tab, b, col));
    return av.localeCompare(bv, undefined, { numeric: true }) * dir;
  });
}

function renderColumnMenu(tab, cols) {
  const menu = document.getElementById('column-menu');
  menu.innerHTML = '';
  defaultColumns[tab].forEach(c => {
    const row = document.createElement('label');
    row.className = 'item';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = cols.includes(c);
    cb.onchange = () => {
      let next = getColumns(tab);
      if (cb.checked) {
        if (!next.includes(c)) next.push(c);
      } else {
        next = next.filter(x => x !== c);
      }
      if (!next.length) next = [defaultColumns[tab][0]];
      setColumns(tab, next);
      render();
    };
    row.appendChild(cb);
    row.appendChild(document.createTextNode(labels[tab][c] || c));
    menu.appendChild(row);
  });
}

function renderDetails(tab, row) {
  const id = document.getElementById('selection-id');
  const sub = document.getElementById('selection-sub');
  const kv = document.getElementById('kv-grid');
  kv.innerHTML = '';

  if (!row) {
    id.textContent = 'None';
    sub.textContent = 'Select a row to view details.';
    drawPacketDonut({ management: 0, control: 0, data: 0, other: 0 });
    return;
  }

  if (tab === 'ap') {
    id.textContent = row.ssid || row.bssid;
    sub.textContent = row.bssid;
    const fields = [
      ['Channel', row.channel ?? '—'], ['Frequency', row.frequency_mhz ?? '—'], ['Band', row.band || 'Unknown'],
      ['Encryption', row.encryption_short || 'Unknown'], ['First Seen', fmtTime(row.first_seen)], ['Last Seen', fmtTime(row.last_seen)],
      ['Clients', row.number_of_clients ?? 0], ['Handshakes', row.handshake_count ?? 0],
    ];
    fields.forEach(([k, v]) => {
      kv.append(elPair(k, String(v)));
    });
  } else if (tab === 'client') {
    id.textContent = row.mac;
    sub.textContent = row.oui_manufacturer || 'Unknown';
    const fields = [
      ['Associated AP', row.associated_ap || '—'], ['RSSI', row.rssi_dbm ?? '—'], ['First Seen', fmtTime(row.first_seen)], ['Last Seen', fmtTime(row.last_seen)],
      ['Data', row.data_transferred_bytes ?? 0], ['Seen APs', (row.seen_access_points || []).length],
      ['Handshake Nets', (row.handshake_networks || []).length], ['Source Adapters', (row.source_adapters || []).join(', ') || '—'],
    ];
    fields.forEach(([k, v]) => kv.append(elPair(k, String(v))));
  } else {
    id.textContent = row.advertised_name || row.alias || row.mac;
    sub.textContent = row.mac;
    const fields = [
      ['Transport', row.transport || 'Unknown'], ['Address Type', row.address_type || '—'], ['RSSI', row.rssi_dbm ?? '—'], ['First Seen', fmtTime(row.first_seen)],
      ['Last Seen', fmtTime(row.last_seen)], ['UUIDs', (row.uuids || []).length], ['Mfgr IDs', (row.mfgr_ids || []).length], ['Adapters', (row.source_adapters || []).join(', ') || '—'],
    ];
    fields.forEach(([k, v]) => kv.append(elPair(k, String(v))));
  }

  drawPacketDonut(packetMix(row));
}

function elPair(k, v) {
  const frag = document.createDocumentFragment();
  const ke = document.createElement('div');
  ke.className = 'k';
  ke.textContent = k;
  const ve = document.createElement('div');
  ve.className = 'v';
  ve.textContent = v;
  frag.append(ke, ve);
  return frag;
}

function renderTabs() {
  const nav = document.querySelector('.tabs');
  nav.innerHTML = '';
  tabs.forEach(t => {
    const b = document.createElement('button');
    b.className = `tab ${activeTab === t.key ? 'active' : ''}`;
    b.textContent = t.label;
    b.onclick = () => {
      activeTab = t.key;
      sortState = { column: null, dir: null };
      selectedKey = null;
      render();
    };
    nav.appendChild(b);
  });
}

function render() {
  renderTabs();

  document.getElementById('aps-count').textContent = (state.access_points || []).length;
  document.getElementById('clients-count').textContent = (state.clients || []).length;
  document.getElementById('scan-state').textContent = state.scanning_wifi || state.scanning_bluetooth ? 'Scanning' : 'Idle';
  document.getElementById('health').textContent = `Wi-Fi: ${state.scanning_wifi ? 'on' : 'off'} | Bluetooth: ${state.scanning_bluetooth ? 'on' : 'off'} | Logs: ${(state.logs || []).length}`;

  const cols = getColumns(activeTab);
  renderColumnMenu(activeTab, cols);
  document.getElementById('table-title').textContent =
    activeTab === 'ap' ? 'Discovered Access Points' : activeTab === 'client' ? 'Discovered Clients & Probes' : 'Discovered Bluetooth Devices';

  let rows = rowsFor(activeTab);
  rows = applyFilters(activeTab, rows, cols);
  rows = applySort(activeTab, rows);

  document.getElementById('found-count').textContent = `${rows.length} found`;

  const wrap = document.getElementById('table-wrap');
  wrap.innerHTML = '';
  const table = document.createElement('table');
  const thead = document.createElement('thead');
  const tbody = document.createElement('tbody');

  const hr = document.createElement('tr');
  cols.forEach(c => {
    const th = document.createElement('th');
    const ind = sortState.column === c ? (sortState.dir === 'asc' ? ' ▲' : sortState.dir === 'desc' ? ' ▼' : '') : '';
    th.textContent = `${labels[activeTab][c] || c}${ind}`;
    th.onclick = () => {
      if (sortState.column !== c) sortState = { column: c, dir: 'asc' };
      else if (sortState.dir === 'asc') sortState = { column: c, dir: 'desc' };
      else if (sortState.dir === 'desc') sortState = { column: null, dir: null };
      else sortState = { column: c, dir: 'asc' };
      render();
    };
    hr.appendChild(th);
  });
  thead.appendChild(hr);

  const fr = document.createElement('tr');
  fr.className = 'filters';
  cols.forEach(c => {
    const th = document.createElement('th');
    const input = document.createElement('input');
    input.value = filters[activeTab][c] || '';
    input.placeholder = labels[activeTab][c] || c;
    input.oninput = (e) => { filters[activeTab][c] = e.target.value; render(); };
    th.appendChild(input);
    fr.appendChild(th);
  });
  thead.appendChild(fr);

  rows.slice(0, 800).forEach(r => {
    const tr = document.createElement('tr');
    const key = rowKey(activeTab, r);
    if (key && key === selectedKey) tr.classList.add('selected');
    tr.onclick = () => { selectedKey = key; render(); };
    cols.forEach(c => {
      const td = document.createElement('td');
      td.textContent = String(valueFor(activeTab, r, c));
      tr.appendChild(td);
    });
    tbody.appendChild(tr);
  });

  table.append(thead, tbody);
  wrap.appendChild(table);

  const selected = rows.find(r => rowKey(activeTab, r) === selectedKey) || rows[0] || null;
  if (selected && !selectedKey) selectedKey = rowKey(activeTab, selected);
  renderDetails(activeTab, selected);

  const scanBtn = document.getElementById('scan-btn');
  const scanning = state.scanning_wifi || state.scanning_bluetooth;
  scanBtn.textContent = scanning ? 'Stop Scan' : 'Start Scan';
  scanBtn.className = `btn ${scanning ? 'destructive' : 'primary'}`;
}

async function refresh() {
  try {
    const res = await fetch('/api/state');
    state = await res.json();
    render();
  } catch (err) {
    document.getElementById('health').textContent = `Backend unavailable: ${err}`;
  }
}

async function post(path) {
  await fetch(path, { method: 'POST' });
  await refresh();
}

document.getElementById('scan-btn').addEventListener('click', async () => {
  const scanning = state.scanning_wifi || state.scanning_bluetooth;
  await post(scanning ? '/api/scan/stop' : '/api/scan/start');
});

document.getElementById('settings-btn').addEventListener('click', () => {
  alert('Preferences migration in progress. Existing backend settings are preserved.');
});

document.getElementById('col-btn').addEventListener('click', () => {
  document.getElementById('column-menu').classList.toggle('open');
});

document.getElementById('global-filter').addEventListener('input', render);

refresh();
setInterval(refresh, 1200);
