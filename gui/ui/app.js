/* mpqtool-gui frontend — talks to the Rust backend through the Tauri API.
 * Mirrors the former egui app: browse (tree + table), preview, extract,
 * repair, import/delete, drag & drop, log. */

'use strict';

const T = window.__TAURI__;
const invoke = (cmd, args) => T.core.invoke(cmd, args || {});

const ARCHIVE_FILTERS = [
  { name: 'MPQ 归档', extensions: ['mpq', 'w3x', 'w3m', 'w3n', 'scx', 'sc2map', 's2ma', 'rep'] },
  { name: '所有文件', extensions: ['*'] },
];
const SYSTEM_DIR = '(系统文件)';
const UNNAMED_DIR = '(未命名)';
const MAX_ROWS = 5000;
const FONT_MIN = 10, FONT_MAX = 28, FONT_DEFAULT = 13;

// ---------------------------------------------------------------------------
// state

const S = {
  /** ArchiveInfoDto from the last successful open (null when not open). */
  info: null,
  /** Path of the file behind the current view, even when parsing failed
   *  (the repair flow works on those bytes). */
  openPath: null,
  cwd: [],               // path segments; [] = archive root
  expanded: new Set(),   // lowercase joined keys of open tree folders
  filter: '',
  selected: new Set(),   // entry keys "classic:12" / "bet:3"
  preview: null,         // { name, text } — floating window
  repair: null,          // { issues, applied, note }
  modify: null,          // { additions, deletions, reencrypt, applied, note }
  settingsOpen: false,
};

// ---------------------------------------------------------------------------
// helpers

const $ = (id) => document.getElementById(id);

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

function fmtBytes(n) {
  return n.toLocaleString('en-US');
}

/** Last `\`/`/`-separated segment of an in-archive path. */
function baseName(name) {
  const parts = name.split(/[\\/]/);
  return parts[parts.length - 1] || name;
}

/** Case-insensitive folder key of a path segment list (like egui). */
function folderKey(path) {
  return path.map((s) => s.toLowerCase()).join('\\');
}

/** Case-insensitive tree lookup, like tasks::node_at. */
function nodeAt(root, path) {
  let node = root;
  for (const seg of path) {
    const lower = seg.toLowerCase();
    node = node.dirs.find((d) => d.name.toLowerCase() === lower);
    if (!node) return null;
  }
  return node;
}

/** Every file entry below `node`, like tasks::collect_files. */
function collectFiles(node) {
  const out = [...node.files];
  for (const d of node.dirs) out.push(...collectFiles(d));
  return out;
}

function collectKeys(node) {
  return collectFiles(node).map((f) => f.key);
}

/** Entries whose full in-archive path contains `needle` (lowercased),
 *  like tasks::search_tree. */
function searchTree(root, needle) {
  const out = [];
  (function walk(node, prefix) {
    for (const d of node.dirs) {
      walk(d, prefix ? `${prefix}\\${d.name}` : d.name);
    }
    for (const f of node.files) {
      const full = f.name ? (prefix ? `${prefix}\\${f.display}` : f.display)
                          : f.display;
      if (full.toLowerCase().includes(needle)) out.push(f);
    }
  })(root, '');
  return out;
}

/** Whether anything below `node` matches the needle (filtered tree). */
function subtreeMatches(node, needle) {
  return searchTree(node, needle).length > 0;
}

function needle() {
  return S.filter.trim().toLowerCase();
}
function searching() {
  return needle() !== '';
}

function modifyFrozen() {
  return !!S.modify && !!S.modify.applied;
}

function basenameOf(path) {
  const parts = String(path).split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** Default repaired-copy name: <stem>.fixed.<ext>. */
function repairedName(path) {
  const base = basenameOf(path);
  const m = base.match(/^(.*?)(\.[^.]*)?$/);
  const ext = m[2] || '.mpq';
  return `${m[1]}.fixed${ext}`;
}

// ---------------------------------------------------------------------------
// log

function log(msg) {
  const el = $('log');
  el.appendChild(document.createTextNode(msg + '\n'));
  el.scrollTop = el.scrollHeight;
}

// ---------------------------------------------------------------------------
// rendering

function renderAll() {
  renderMenu();
  renderStatus();
  renderMain();
  renderModals();
  renderLogButtons();
}

function renderMenu() {
  const hasArchive = !!S.info;
  const hasPath = !!S.openPath;
  $('m-extract-all').disabled = !hasArchive;
  $('m-import').disabled = !hasArchive || modifyFrozen();
  $('m-reload').disabled = !hasPath;
  $('m-repair').disabled = !hasPath;
  $('menu-path').textContent = S.openPath || '未打开归档';
}

function renderStatus() {
  const el = $('statusbar');
  if (S.info) {
    const i = S.info;
    const parts = [
      `<span class="strong">MPQ v${i.version}</span>`,
      `<span>扇区 ${fmtBytes(i.sector_size)} B</span>`,
      `<span>hash ${fmtBytes(i.hash_table_size)} / block ${fmtBytes(i.block_table_size)}</span>`,
    ];
    if (i.het_bet) parts.push('<span class="strong">+ HET/BET</span>');
    parts.push(`<span>listfile ${fmtBytes(i.listfile_count)} 项</span>`,
               `<span>警告 ${i.warnings.length}</span>`);
    el.innerHTML = parts.join('<span class="sep">|</span>');
  } else {
    el.innerHTML = '<span>打开一个 .mpq / .w3x / .sc2map 文件开始</span>';
  }
}

function renderMain() {
  const browsing = !!S.info;
  $('welcome').hidden = browsing;
  $('browser').hidden = !browsing;
  if (browsing) {
    renderTree();
    renderToolbar();
    renderBreadcrumb();
    renderTable();
  }
}

function renderLogButtons() { /* static */ }

// -- tree --------------------------------------------------------------------

function renderTree() {
  const root = S.info.tree;
  const hdr = $('tree-header');
  hdr.textContent = `文件夹（${fmtBytes(root.file_count)} 项）`;

  const treeEl = $('tree');
  treeEl.replaceChildren();

  const rootRow = document.createElement('div');
  rootRow.className = 'tree-root-row';
  const rootBtn = document.createElement('button');
  rootBtn.className = 'tree-node' + (S.cwd.length === 0 && !searching() ? ' active' : '');
  rootBtn.textContent = '🏠 归档根目录';
  rootBtn.title = '回到归档顶层';
  rootBtn.onclick = () => { S.cwd = []; renderTree(); renderToolbar(); renderBreadcrumb(); renderTable(); };
  rootRow.appendChild(rootBtn);
  treeEl.appendChild(rootRow);

  for (const d of root.dirs) {
    appendTreeNode(treeEl, d, [d.name]);
  }
}

function appendTreeNode(parent, node, path) {
  const n = needle();
  if (n && !subtreeMatches(node, n)) return;

  const key = folderKey(path);
  const open = n !== '' || S.expanded.has(key);
  const icon = node.name === SYSTEM_DIR ? '🗂' : node.name === UNNAMED_DIR ? '❔' : '📁';
  const arrow = node.dirs.length === 0 ? ' ' : open ? '▾' : '▸';

  const btn = document.createElement('button');
  btn.className = 'tree-node' + (S.cwd.join('\\') === path.join('\\') && !n ? ' active' : '');
  btn.style.paddingLeft = `${(path.length - 1) * 14 + 8}px`;
  btn.textContent = `${arrow} ${icon} ${node.name} (${fmtBytes(node.file_count)})`;
  btn.onclick = () => {
    S.cwd = [...path];
    if (node.dirs.length && !n) {
      if (S.expanded.has(key)) S.expanded.delete(key); else S.expanded.add(key);
    }
    renderTree(); renderToolbar(); renderBreadcrumb(); renderTable();
  };
  parent.appendChild(btn);

  if (open) {
    for (const d of node.dirs) appendTreeNode(parent, d, [...path, d.name]);
  }
}

// -- toolbar / breadcrumb ------------------------------------------------------

function renderToolbar() {
  $('b-extract-sel').disabled = S.selected.size === 0;
  $('b-delete-sel').disabled = S.selected.size === 0 || modifyFrozen();
  $('b-clear-filter').hidden = S.filter === '';
  $('sel-count').textContent = `已选 ${fmtBytes(S.selected.size)} 项`;
}

function renderBreadcrumb() {
  const el = $('breadcrumb');
  el.replaceChildren();

  const refresh = document.createElement('button');
  refresh.textContent = '🔄';
  refresh.title = '重新从磁盘载入当前归档（F5）；外部修改或保存后可用来同步视图';
  refresh.onclick = reloadArchive;
  el.appendChild(refresh);

  const sep = document.createElement('span');
  sep.className = 'sep';
  sep.textContent = '|';
  el.appendChild(sep);

  if (searching()) {
    const n = searchTree(S.info.tree, needle()).length;
    const strong = document.createElement('span');
    strong.className = 'searching';
    strong.textContent = `搜索 “${S.filter.trim()}”：${fmtBytes(n)} 个匹配`;
    el.appendChild(strong);
    return;
  }

  const up = document.createElement('button');
  up.textContent = '⬆ 上一级';
  up.disabled = S.cwd.length === 0;
  up.onclick = () => { S.cwd = S.cwd.slice(0, -1); navRender(); };
  el.appendChild(up);

  const home = document.createElement('button');
  home.textContent = '🏠';
  home.onclick = () => { S.cwd = []; navRender(); };
  el.appendChild(home);

  S.cwd.forEach((seg, i) => {
    const sep = document.createElement('span');
    sep.className = 'sep';
    sep.textContent = '›';
    el.appendChild(sep);
    const b = document.createElement('button');
    b.className = 'crumb-seg';
    b.textContent = seg;
    b.onclick = () => { S.cwd = S.cwd.slice(0, i + 1); navRender(); };
    el.appendChild(b);
  });

  const node = nodeAt(S.info.tree, S.cwd);
  if (node) {
    const w = document.createElement('span');
    w.className = 'weak';
    w.textContent = `（${fmtBytes(node.dirs.length)} 个子文件夹，${fmtBytes(node.files.length)} 个文件）`;
    el.appendChild(w);
  }
}

function navRender() {
  renderTree(); renderToolbar(); renderBreadcrumb(); renderTable();
}

// -- file table ----------------------------------------------------------------

/** Rows currently shown: {type:'dir', node, path} | {type:'file', entry, showFull}. */
let currentRows = [];

function renderTable() {
  currentRows = [];
  if (searching()) {
    for (const e of searchTree(S.info.tree, needle())) {
      currentRows.push({ type: 'file', entry: e, showFull: true });
    }
  } else {
    const node = nodeAt(S.info.tree, S.cwd);
    if (node) {
      for (const d of node.dirs) currentRows.push({ type: 'dir', node: d, path: [...S.cwd, d.name] });
      for (const f of node.files) currentRows.push({ type: 'file', entry: f, showFull: false });
    }
  }

  const truncated = currentRows.length > MAX_ROWS;
  const rows = currentRows.slice(0, MAX_ROWS);
  const parts = [];
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    if (r.type === 'dir') {
      const keys = collectKeys(r.node);
      const all = keys.length > 0 && keys.every((k) => S.selected.has(k));
      parts.push(
        `<tr data-ri="${i}">` +
        `<td class="col-check"><input type="checkbox" data-dir="${i}" ${all ? 'checked' : ''} title="选中此文件夹下的全部文件"></td>` +
        `<td class="cname cdir" title="双击进入文件夹">📁 ${esc(r.node.name)}（${fmtBytes(r.node.file_count)} 项）</td>` +
        `<td class="col-num">${fmtBytes(r.node.total_size)}</td>` +
        `<td class="col-num">${fmtBytes(r.node.total_csize)}</td>` +
        `<td class="col-flags"></td><td class="col-time"></td><td class="col-num"></td><td class="col-src"></td>` +
        `</tr>`,
      );
    } else {
      const e = r.entry;
      const named = !!e.name;
      const label = r.showFull ? (named ? e.name : e.display)
                               : (named ? baseName(e.name) : e.display);
      const cls = named ? 'cname' : 'cname unnamed';
      const title = named ? '' : ' title="未命名文件无法预览（缺少名字无法解密/定位）"';
      parts.push(
        `<tr data-ri="${i}">` +
        `<td class="col-check"><input type="checkbox" data-key="${esc(e.key)}" ${S.selected.has(e.key) ? 'checked' : ''}></td>` +
        `<td class="${cls}"${title}>${named ? esc(label) : '⚠ ' + esc(label)}</td>` +
        `<td class="col-num">${fmtBytes(e.size)}</td>` +
        `<td class="col-num">${fmtBytes(e.csize)}</td>` +
        `<td class="col-flags">${esc(e.flags)}</td>` +
        `<td class="col-time">${e.mtime ? esc(e.mtime) : '—'}</td>` +
        `<td class="col-num">${e.locale}</td>` +
        `<td class="col-src">${e.source === 'classic' ? 'blk' : 'bet'}</td>` +
        `</tr>`,
      );
    }
  }
  const tbody = document.querySelector('#file-table tbody');
  tbody.innerHTML = parts.join('');
  $('table-note').textContent = truncated
    ? `（结果过多，仅显示前 ${MAX_ROWS} 项，请用过滤缩小范围）` : '';
}

function tableChanged(ev) {
  const t = ev.target;
  if (t.tagName !== 'INPUT') return;
  if (t.dataset.key !== undefined) {
    if (t.checked) S.selected.add(t.dataset.key); else S.selected.delete(t.dataset.key);
  } else if (t.dataset.dir !== undefined) {
    const keys = collectKeys(currentRows[+t.dataset.dir].node);
    for (const k of keys) {
      if (t.checked) S.selected.add(k); else S.selected.delete(k);
    }
  }
  renderToolbar();
  renderTable();
}

function tableDblClick(ev) {
  const tr = ev.target.closest('tr');
  if (!tr || tr.dataset.ri === undefined) return;
  const r = currentRows[+tr.dataset.ri];
  if (r.type === 'dir') {
    S.cwd = r.path;
    navRender();
  } else if (r.entry.name) {
    previewFile(r.entry.name);
  } else {
    log('未命名文件无法预览（缺少名字无法解密/定位）');
  }
}

// ---------------------------------------------------------------------------
// open / reload / drop

function applyOpenResult(res, pathIfLoaded, { keepNav = false } = {}) {
  entryByKey = null;
  if (res.note) log(res.note);
  if (res.ok) {
    S.info = res.info;
    S.openPath = res.info.path;
    if (!keepNav) {
      S.cwd = [];
      S.expanded.clear();
      S.selected.clear();
    }
    log(`已打开 ${res.info.path}（${fmtBytes(res.info.entry_count)} 个条目）`);
    for (const w of res.info.warnings) log(`warning: ${w}`);
  } else {
    // The backend recorded the bytes (only the parse failed), so the
    // repair flow remains available.
    if (pathIfLoaded !== undefined) S.openPath = pathIfLoaded;
    S.info = null;
    if (!keepNav) {
      S.cwd = [];
      S.expanded.clear();
      S.selected.clear();
    }
    log(`打开失败：${res.error}`);
    log('可以尝试「工具 → 修复头信息」。');
  }
  renderAll();
}

async function openArchiveDialog() {
  const path = await T.dialog.open({ title: '打开 MPQ 归档', filters: ARCHIVE_FILTERS });
  if (!path) return;
  await openArchivePath(path);
}

async function openArchivePath(path) {
  try {
    const res = await invoke('open_archive', { path });
    applyOpenResult(res, path);
  } catch (e) {
    log(String(e)); // e.g. 读取失败：…
  }
}

async function reloadArchive() {
  if (!S.openPath) return;
  const hadPending = !!S.modify && !S.modify.applied &&
    (S.modify.additions.length > 0 || S.modify.deletions.length > 0);
  const cwd = S.cwd, expanded = S.expanded, filter = S.filter;
  try {
    const res = await invoke('reload_archive');
    // keep navigation on reload (like the egui refresh)
    applyOpenResult(res, S.openPath, { keepNav: true });
    S.cwd = cwd; S.expanded = expanded; S.filter = filter;
    S.selected.clear();
    S.preview = null;
    renderPreviewWindow();
    if (hadPending) S.modify = null;
    if (res.ok) {
      log('（已从磁盘刷新）');
      if (hadPending) log('（刷新已清空未保存的「修改归档」待办）');
    }
    $('filter').value = S.filter;
    renderAll();
  } catch (e) {
    log(String(e)); // 刷新失败：…（当前仍显示已载入的内容）
  }
}

async function handleDrop(paths) {
  if (S.info) {
    log(`（通过拖拽导入 ${fmtBytes(paths.length)} 个文件）`);
    await queueImports(paths);
    return;
  }
  try {
    const res = await invoke('open_archive', { path: paths[0] });
    applyOpenResult(res, paths[0]);
    if (res.ok) {
      log('（通过拖拽打开）');
      if (paths.length > 1) {
        log(`（忽略了另外 ${fmtBytes(paths.length - 1)} 个拖入的文件，一次只能打开一个归档）`);
      }
    }
  } catch (e) {
    log(String(e));
  }
}

function showDropOverlay(count) {
  const el = $('drop-overlay');
  el.innerHTML = `<div class="banner">${
    S.info ? `📥 松开鼠标以导入 ${fmtBytes(count)} 个文件到当前归档`
           : '📂 松开鼠标以打开归档'}</div>`;
  el.hidden = false;
}
function hideDropOverlay() {
  $('drop-overlay').hidden = true;
}

// ---------------------------------------------------------------------------
// preview / extract

async function previewFile(name) {
  try {
    const p = await invoke('preview_file', { name });
    const note = p.truncated
      ? `（仅显示前 256 字节，共 ${fmtBytes(p.size)} 字节）`
      : `（${fmtBytes(p.size)} 字节）`;
    S.preview = { name: `${name} ${note}`, text: p.text };
    renderPreviewWindow();
  } catch (e) {
    log(`预览失败 ${name}: ${e}`);
  }
}

async function pickFolder(title) {
  return T.dialog.open({ title, directory: true });
}

async function extractSelected() {
  if (S.selected.size === 0) { log('未选择任何文件。'); return; }
  const dir = await pickFolder('选择输出目录');
  if (!dir) return;
  await runExtract([...S.selected], dir);
}

async function extractAll() {
  const dir = await pickFolder('选择输出目录');
  if (!dir) return;
  await runExtract([], dir);
}

async function runExtract(keys, dir) {
  try {
    const r = await invoke('extract', { keys, dir });
    for (const m of r.messages) log(m);
    log(`提取完成：${fmtBytes(r.ok)} 成功，${fmtBytes(r.failed)} 失败 → ${dir}`);
  } catch (e) {
    log(String(e));
  }
}

// ---------------------------------------------------------------------------
// selection

function selectAll() {
  let all;
  if (searching()) {
    all = searchTree(S.info.tree, needle());
  } else {
    const node = nodeAt(S.info.tree, S.cwd);
    all = node ? collectFiles(node) : [];
  }
  for (const f of all) S.selected.add(f.key);
  renderToolbar();
  renderTable();
}

function selectNone() {
  S.selected.clear();
  renderToolbar();
  renderTable();
}

// ---------------------------------------------------------------------------
// import / delete (修改归档 window)

function defaultModify() {
  return { additions: [], deletions: [], reencrypt: true, applied: null, note: '' };
}

async function importFilesDialog() {
  const paths = await T.dialog.open({ title: '选择要导入的文件', multiple: true });
  if (!paths) return;
  await queueImports(Array.isArray(paths) ? paths : [paths]);
}

/** Queue files for import; mirrors the egui queue_imports flow. */
async function queueImports(paths) {
  if (!paths.length) return;
  if (modifyFrozen()) {
    log('已有应用过的修改待保存或关闭，请先处理「修改归档」窗口。');
    renderModals();
    return;
  }
  S.modify ??= defaultModify();

  let stats;
  try {
    stats = await invoke('stat_imports', { paths });
  } catch (e) {
    log(String(e));
    return;
  }

  let queued = 0;
  paths.forEach((path, i) => {
    const st = stats[i];
    if (!st) return;
    if (st.error) {
      log(`读取失败 ${path}: ${st.error}`);
      return;
    }
    // Same-name import supersedes an earlier queued one; replaces when it
    // hits an existing entry or an already-queued import.
    const dupIdx = S.modify.additions.findIndex((a) => a.name.toLowerCase() === st.name.toLowerCase());
    const replaces = st.replaces || dupIdx >= 0;
    log(`待导入 ${st.name}（${fmtBytes(st.size)} 字节，${replaces ? '替换已有文件' : '新增'}）`);
    const item = { name: st.name, path, size: st.size, replaces };
    if (dupIdx >= 0) S.modify.additions[dupIdx] = item;
    else S.modify.additions.push(item);
    queued++;
  });
  if (queued > 0) {
    log(`已加入导入队列（${fmtBytes(queued)} 项），在「修改归档」窗口中应用。`);
  }
  renderMenu();
  renderModals();
}

function deleteSelected() {
  if (S.selected.size === 0) { log('未选择任何文件。'); return; }
  if (modifyFrozen()) return;
  S.modify ??= defaultModify();

  // egui iterates a BTreeSet<(source, block_index)>: classic before bet,
  // then numeric index.
  const sorted = [...S.selected].sort((a, b) => {
    const [sa, ia] = a.split(':'), [sb, ib] = b.split(':');
    return sa === sb ? (+ia) - (+ib) : sa.localeCompare(sb);
  });
  let queued = 0;
  for (const key of sorted) {
    if (S.modify.deletions.some((d) => d.key === key)) continue;
    const entry = findEntryByKey(key);
    if (!entry) continue;
    S.modify.deletions.push({
      display: entry.name || entry.display,
      name: entry.name || null,
      key,
    });
    queued++;
  }
  if (queued > 0) {
    log(`已标记 ${fmtBytes(queued)} 项待删除，在「修改归档」窗口中应用。`);
  }
  renderMenu();
  renderModals();
}

/** Flatten the tree into a key → entry map (built on demand). */
let entryByKey = null;
function findEntryByKey(key) {
  if (!entryByKey) {
    entryByKey = new Map();
    (function walk(node) {
      for (const d of node.dirs) walk(d);
      for (const f of node.files) entryByKey.set(f.key, f);
    })(S.info.tree);
  }
  return entryByKey.get(key) || null;
}

async function applyModify() {
  const m = S.modify;
  try {
    const r = await invoke('apply_modify', {
      additions: m.additions.map((a) => ({ name: a.name, path: a.path })),
      deletions: m.deletions.map((d) => ({ name: d.name, key: d.name ? null : d.key })),
      reencrypt: m.reencrypt,
    });
    m.applied = r.note;
    m.note = r.note;
  } catch (e) {
    m.note = `修改失败：${e}`;
  }
  renderMenu();
  renderModals();
}

async function saveModified() {
  const suggested = basenameOf(S.openPath || '');
  const out = await T.dialog.save({ title: '保存修改后的归档', defaultPath: suggested });
  if (!out) return;
  try {
    const res = await invoke('save_modified', { path: out });
    log(`修改已保存到 ${out}`);
    S.modify = null;
    invoke('discard_modify').catch(() => {});
    applyOpenResult(res);
  } catch (e) {
    S.modify.note = String(e); // 保存失败：…
    renderModals();
  }
}

async function openModified() {
  try {
    const res = await invoke('open_modified');
    S.modify = null;
    invoke('discard_modify').catch(() => {});
    applyOpenResult(res);
  } catch (e) {
    log(String(e));
  }
}

// ---------------------------------------------------------------------------
// repair (修复头信息 window)

async function startRepair() {
  if (!S.openPath) { log('请先打开一个归档。'); return; }
  try {
    const r = await invoke('diagnose_archive');
    S.repair = {
      issues: r.issues,
      applied: false,
      note: r.issues.length === 0 ? '未发现头信息问题。' : '',
    };
    renderModals();
  } catch (e) {
    log(`诊断失败：${e}`);
  }
}

async function applyRepair() {
  try {
    const r = await invoke('apply_repair');
    S.repair.applied = true;
    S.repair.note = r.note;
  } catch (e) {
    S.repair.note = `修复失败：${e}`;
  }
  renderModals();
}

async function saveRepaired() {
  const suggested = repairedName(S.openPath || 'archive.mpq');
  const out = await T.dialog.save({ title: '保存修复后的归档', defaultPath: suggested });
  if (!out) return;
  try {
    const r = await invoke('save_repaired', { path: out });
    S.repair.note = r.note;
    log(`修复副本已保存到 ${out}`);
    renderModals();
  } catch (e) {
    S.repair.note = String(e); // 保存失败：…
    renderModals();
  }
}

async function openRepaired() {
  try {
    const res = await invoke('open_repaired');
    S.repair = null;
    invoke('discard_repair').catch(() => {});
    applyOpenResult(res);
  } catch (e) {
    log(String(e));
  }
}

// ---------------------------------------------------------------------------
// windows & modals rendering

function renderPreviewWindow() {
  const host = $('windows');
  host.replaceChildren();
  if (!S.preview) return;

  const win = document.createElement('div');
  win.className = 'fwindow';
  win.style.left = '90px';
  win.style.top = '70px';
  win.innerHTML =
    `<div class="fwin-title"><span>预览</span><button class="close" title="关闭">✕</button></div>` +
    `<div class="fwin-body"><strong></strong><pre></pre></div>`;
  win.querySelector('strong').textContent = S.preview.name;
  win.querySelector('pre').textContent = S.preview.text;
  win.querySelector('.close').onclick = () => { S.preview = null; host.replaceChildren(); };

  // drag by title bar
  const title = win.querySelector('.fwin-title');
  let sx = 0, sy = 0, ox = 0, oy = 0, dragging = false;
  title.addEventListener('pointerdown', (ev) => {
    if (ev.target.closest('.close')) return;
    dragging = true;
    sx = ev.clientX; sy = ev.clientY;
    ox = win.offsetLeft; oy = win.offsetTop;
    title.setPointerCapture(ev.pointerId);
  });
  title.addEventListener('pointermove', (ev) => {
    if (!dragging) return;
    win.style.left = `${Math.max(0, ox + ev.clientX - sx)}px`;
    win.style.top = `${Math.max(0, oy + ev.clientY - sy)}px`;
  });
  title.addEventListener('pointerup', () => { dragging = false; });

  host.appendChild(win);
}

function renderModals() {
  const host = $('modals');
  host.replaceChildren();
  if (S.repair) host.appendChild(buildRepairModal());
  if (S.modify) host.appendChild(buildModifyModal());
  if (S.settingsOpen) host.appendChild(buildSettingsModal());
}

function modalShell(title, bodyEl, actionsEl, note) {
  const backdrop = document.createElement('div');
  backdrop.className = 'modal-backdrop';
  const modal = document.createElement('div');
  modal.className = 'modal';
  const head = document.createElement('div');
  head.className = 'modal-title';
  head.innerHTML = `<span>${esc(title)}</span><button class="close" title="关闭">✕</button>`;
  modal.appendChild(head);
  const body = document.createElement('div');
  body.className = 'modal-body';
  if (bodyEl) body.appendChild(bodyEl);
  modal.appendChild(body);
  if (actionsEl) {
    const actions = document.createElement('div');
    actions.className = 'modal-actions';
    actions.appendChild(actionsEl);
    modal.appendChild(actions);
  }
  if (note) {
    const n = document.createElement('div');
    n.className = 'modal-note';
    n.textContent = note;
    modal.appendChild(n);
  }
  backdrop.appendChild(modal);
  return backdrop;
}

function buildRepairModal() {
  const r = S.repair;

  const body = document.createElement('div');
  const src = document.createElement('div');
  src.textContent = `来源：${S.openPath || ''}`;
  body.appendChild(src);

  const hr = () => { const h = document.createElement('hr'); h.style.border = 'none'; h.style.borderTop = '1px solid var(--border)'; return h; };
  body.appendChild(hr());

  if (r.issues.length === 0) {
    const p = document.createElement('div');
    p.textContent = '未发现头信息问题。';
    body.appendChild(p);
  } else {
    const list = document.createElement('div');
    list.className = 'issue-list';
    for (const issue of r.issues) {
      const row = document.createElement('div');
      row.className = 'issue';
      if (issue.action) {
        row.innerHTML =
          `<span class="tag fixable">可修复</span>` +
          `<span class="field"></span>` +
          `<span class="desc"></span>`;
        row.querySelector('.desc').textContent = `${issue.problem} → ${issue.action}`;
      } else {
        row.innerHTML =
          `<span class="tag fatal">无法修复</span>` +
          `<span class="field"></span>` +
          `<span class="desc"></span>`;
        row.querySelector('.desc').textContent = issue.problem;
      }
      row.querySelector('.field').textContent = issue.field;
      list.appendChild(row);
    }
    const scroll = document.createElement('div');
    scroll.style.maxHeight = '260px';
    scroll.style.overflow = 'auto';
    scroll.appendChild(list);
    body.appendChild(scroll);
    body.appendChild(hr());
  }

  const actions = document.createElement('div');
  actions.style.display = 'contents';
  const mk = (text, disabled, onclick, title) => {
    const b = document.createElement('button');
    b.textContent = text;
    b.disabled = disabled;
    if (title) b.title = title;
    b.onclick = onclick;
    return b;
  };
  actions.appendChild(mk('应用修复', r.applied || r.issues.length === 0, applyRepair));
  actions.appendChild(mk('保存修复副本…', !r.applied, saveRepaired));
  actions.appendChild(mk('在界面中打开修复结果', !r.applied, openRepaired));
  actions.appendChild(mk('关闭', false, () => {
    S.repair = null;
    invoke('discard_repair').catch(() => {});
    renderModals();
  }));

  return modalShell('修复头信息', body, actions, r.note || '');
}

function buildModifyModal() {
  const m = S.modify;
  const frozen = !!m.applied;

  const body = document.createElement('div');

  const src = document.createElement('div');
  src.textContent = `来源：${S.openPath || '（未保存）'}`;
  body.appendChild(src);
  const hr = () => { const h = document.createElement('hr'); h.style.border = 'none'; h.style.borderTop = '1px solid var(--border)'; return h; };
  body.appendChild(hr());

  const scroll = document.createElement('div');
  scroll.style.maxHeight = '280px';
  scroll.style.overflow = 'auto';
  scroll.style.display = 'flex';
  scroll.style.flexDirection = 'column';
  scroll.style.gap = '6px';

  const addLabel = document.createElement('div');
  addLabel.className = 'section-label';
  addLabel.textContent = `待导入（${fmtBytes(m.additions.length)} 项）`;
  scroll.appendChild(addLabel);
  if (m.additions.length === 0) {
    const w = document.createElement('div');
    w.className = 'hint-text';
    w.textContent = '（无）— 用「文件 → 导入文件到归档…」添加';
    scroll.appendChild(w);
  } else {
    const list = document.createElement('div');
    list.className = 'pending-list';
    m.additions.forEach((a, i) => {
      const row = document.createElement('div');
      row.className = 'pending-row';
      row.innerHTML =
        `<span class="tag ${a.replaces ? 'replace' : ''}">${a.replaces ? '替换' : '新增'}</span>` +
        `<span class="name"></span><span class="meta">${fmtBytes(a.size)} 字节</span>`;
      row.querySelector('.name').textContent = a.name;
      if (!frozen) {
        const x = document.createElement('button');
        x.className = 'remove';
        x.textContent = '✕';
        x.onclick = () => { m.additions.splice(i, 1); renderModals(); };
        row.appendChild(x);
      }
      list.appendChild(row);
    });
    scroll.appendChild(list);
  }

  const delLabel = document.createElement('div');
  delLabel.className = 'section-label';
  delLabel.style.marginTop = '8px';
  delLabel.textContent = `待删除（${fmtBytes(m.deletions.length)} 项）`;
  scroll.appendChild(delLabel);
  if (m.deletions.length === 0) {
    const w = document.createElement('div');
    w.className = 'hint-text';
    w.textContent = '（无）— 勾选文件后点「删除选中」';
    scroll.appendChild(w);
  } else {
    const list = document.createElement('div');
    list.className = 'pending-list';
    m.deletions.forEach((d, i) => {
      const row = document.createElement('div');
      row.className = 'pending-row';
      row.innerHTML =
        `<span class="tag">删除</span><span class="name"></span>`;
      row.querySelector('.name').textContent = d.display;
      if (!frozen) {
        const x = document.createElement('button');
        x.className = 'remove';
        x.textContent = '✕';
        x.onclick = () => { m.deletions.splice(i, 1); renderModals(); };
        row.appendChild(x);
      }
      list.appendChild(row);
    });
    scroll.appendChild(list);
  }

  const hint = document.createElement('div');
  hint.className = 'hint-text';
  hint.style.marginTop = '4px';
  hint.textContent = '应用时保留文件将原样拷贝；(listfile) 重建，(attributes)/(signature) 自动移除。';
  scroll.appendChild(hint);
  body.appendChild(scroll);
  body.appendChild(hr());

  if (!frozen) {
    const label = document.createElement('label');
    label.style.display = 'flex';
    label.style.gap = '6px';
    label.style.alignItems = 'center';
    label.style.cursor = 'pointer';
    label.title = 'key-v2 的密钥包含块偏移，修改后必须重算。勾选：用新偏移的密钥重新加密（保持加密状态）；\n取消：存为未加密（内容不变，任何工具都能读）';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = m.reencrypt;
    cb.onchange = () => { m.reencrypt = cb.checked; };
    label.appendChild(cb);
    label.appendChild(document.createTextNode('key-v2 文件按新偏移重新加密'));
    body.appendChild(label);
  }

  const actions = document.createElement('div');
  actions.style.display = 'contents';
  const mk = (text, disabled, onclick, title) => {
    const b = document.createElement('button');
    b.textContent = text;
    b.disabled = disabled;
    if (title) b.title = title;
    b.onclick = onclick;
    return b;
  };
  actions.appendChild(mk('应用修改',
    frozen || (m.additions.length === 0 && m.deletions.length === 0),
    applyModify));
  actions.appendChild(mk('保存到文件…', !frozen, saveModified,
    '选择保存位置（默认覆盖原文件）后自动重新载入'));
  actions.appendChild(mk('在界面中打开结果', !frozen, openModified,
    '不写盘，直接在界面中浏览修改结果'));
  actions.appendChild(mk('关闭', false, () => {
    S.modify = null;
    invoke('discard_modify').catch(() => {});
    renderMenu();
    renderModals();
  }));

  return modalShell('修改归档', body, actions, m.note || '');
}

function buildSettingsModal() {
  const body = document.createElement('div');

  const heading = document.createElement('div');
  heading.className = 'section-label';
  heading.textContent = '字体大小';
  body.appendChild(heading);

  const slider = document.createElement('input');
  slider.type = 'range';
  slider.min = FONT_MIN;
  slider.max = FONT_MAX;
  slider.step = 0.5;
  slider.value = currentFontSize();
  const apply = (v) => {
    document.documentElement.style.fontSize = `${v}px`;
    localStorage.setItem('font_size', String(v));
  };
  slider.oninput = () => apply(parseFloat(slider.value));
  body.appendChild(slider);

  const presets = document.createElement('div');
  presets.className = 'preset-row';
  const label = document.createElement('span');
  label.textContent = '预设：';
  presets.appendChild(label);
  for (const [text, size] of [['小', 11], ['中', 13], ['大', 16], ['特大', 19]]) {
    const b = document.createElement('button');
    b.textContent = text;
    if (Math.abs(currentFontSize() - size) < 0.01) b.classList.add('active');
    b.onclick = () => { apply(size); slider.value = size; renderModals(); };
    presets.appendChild(b);
  }
  const reset = document.createElement('button');
  reset.textContent = '恢复默认';
  reset.onclick = () => { apply(FONT_DEFAULT); slider.value = FONT_DEFAULT; renderModals(); };
  presets.appendChild(reset);
  body.appendChild(presets);

  const preview = document.createElement('div');
  preview.className = 'settings-preview';
  preview.innerHTML = `<div>预览：MPQ 归档浏览器 — 文件列表 ABCabc 0123456789</div>` +
                      `<div class="mono">预览（等宽）：war3map.j  SECTOR=4096</div>`;
  body.appendChild(preview);

  const hint = document.createElement('div');
  hint.className = 'hint-text';
  hint.textContent = '字号立即应用于整个界面，并自动保存。';
  body.appendChild(hint);

  const actions = document.createElement('div');
  actions.style.display = 'contents';
  const close = document.createElement('button');
  close.textContent = '关闭';
  close.onclick = () => { S.settingsOpen = false; renderModals(); };
  actions.appendChild(close);

  const modal = modalShell('设置', body, actions, '');
  modal.firstChild.id = 'modal-settings';
  return modal;
}

function currentFontSize() {
  const v = parseFloat(localStorage.getItem('font_size'));
  return Number.isFinite(v) ? Math.min(FONT_MAX, Math.max(FONT_MIN, v)) : FONT_DEFAULT;
}

// ---------------------------------------------------------------------------
// menus

function initMenus() {
  document.querySelectorAll('.menu-title').forEach((title) => {
    title.addEventListener('click', (ev) => {
      ev.stopPropagation();
      const root = title.closest('.menu-root');
      const wasOpen = root.classList.contains('open');
      closeMenus();
      if (!wasOpen) {
        root.classList.add('open');
        title.classList.add('open');
      }
    });
  });
  document.addEventListener('click', (ev) => {
    if (!ev.target.closest('.menu-root')) closeMenus();
  });
}

function closeMenus() {
  document.querySelectorAll('.menu-root.open').forEach((root) => {
    root.classList.remove('open');
    root.querySelector('.menu-title').classList.remove('open');
  });
}

/** Wire a menu-dropdown button: toggle-aware click handler. */
function menuAction(id, handler) {
  $(id).addEventListener('click', (ev) => {
    ev.stopPropagation();
    closeMenus();
    handler();
  });
}

// ---------------------------------------------------------------------------
// init

function initTreeSplitter() {
  const splitter = $('tree-splitter');
  const panel = $('tree-panel');
  splitter.addEventListener('pointerdown', (ev) => {
    splitter.classList.add('dragging');
    splitter.setPointerCapture(ev.pointerId);
    const move = (e) => {
      const rect = $('browser').getBoundingClientRect();
      const w = Math.min(Math.max(e.clientX - rect.left, 150), Math.max(200, rect.width * 0.6));
      panel.style.width = `${w}px`;
    };
    const up = () => {
      splitter.classList.remove('dragging');
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  });
}

async function initDragDrop() {
  try {
    const webview = T.webview.getCurrentWebview();
    await webview.onDragDropEvent((event) => {
      const p = event.payload;
      if (p.type === 'enter' || p.type === 'over') {
        showDropOverlay((p.paths || []).length);
      } else if (p.type === 'drop') {
        hideDropOverlay();
        handleDrop(p.paths || []);
      } else if (p.type === 'leave' || p.type === 'exit') {
        hideDropOverlay();
      }
    });
  } catch (e) {
    // drag & drop is a convenience; never fatal
    console.warn('onDragDropEvent unavailable:', e);
  }
}

function init() {
  document.documentElement.style.fontSize = `${currentFontSize()}px`;

  initMenus();
  initTreeSplitter();
  initDragDrop();

  menuAction('m-open', openArchiveDialog);
  $('w-open').onclick = openArchiveDialog;
  menuAction('m-extract-all', extractAll);
  menuAction('m-import', importFilesDialog);
  menuAction('m-reload', reloadArchive);
  menuAction('m-quit', () => T.window.getCurrentWindow().close());
  menuAction('m-repair', startRepair);
  menuAction('m-settings', () => { S.settingsOpen = true; renderModals(); });

  $('b-extract-sel').onclick = extractSelected;
  $('b-delete-sel').onclick = deleteSelected;
  $('b-select-all').onclick = selectAll;
  $('b-select-none').onclick = selectNone;
  $('b-clear-log').onclick = () => { $('log').replaceChildren(); };

  let filterTimer = null;
  $('filter').addEventListener('input', (ev) => {
    S.filter = ev.target.value;
    $('b-clear-filter').hidden = S.filter === '';
    clearTimeout(filterTimer);
    filterTimer = setTimeout(() => {
      renderTree();
      renderToolbar();
      renderBreadcrumb();
      renderTable();
    }, 120);
  });
  $('b-clear-filter').onclick = () => {
    S.filter = '';
    $('filter').value = '';
    $('b-clear-filter').hidden = true;
    renderTree(); renderToolbar(); renderBreadcrumb(); renderTable();
  };

  document.querySelector('#file-table tbody').addEventListener('change', tableChanged);
  document.querySelector('#file-table tbody').addEventListener('dblclick', tableDblClick);

  document.addEventListener('keydown', (ev) => {
    if (ev.key === 'F5') {
      ev.preventDefault();
      reloadArchive();
    }
  });

  renderAll();
}

document.addEventListener('DOMContentLoaded', init);
