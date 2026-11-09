import {
  ConnectionState,
  FileTransferItem,
  HostStatusSnapshot,
  MonitorRect,
  SavedRemote,
  SessionSnapshot,
  TransferDirection,
  UpdateState,
  UpdateStatusSnapshot,
} from '../../domain/models/session.model';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function asString(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback;
}

function asNumber(value: unknown, fallback = 0): number {
  const num = Number(value);
  return Number.isFinite(num) ? num : fallback;
}

function asBoolean(value: unknown, fallback = false): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

function parseMonitorRect(item: unknown): MonitorRect | null {
  if (!Array.isArray(item) || item.length < 4) {
    return null;
  }
  const l = asNumber(item[0], 0);
  const t = asNumber(item[1], 0);
  const r = asNumber(item[2], 0);
  const b = asNumber(item[3], 0);
  return [l, t, r, b];
}

function parseMonitors(value: unknown): MonitorRect[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const out: MonitorRect[] = [];
  for (const item of value) {
    const rect = parseMonitorRect(item);
    if (rect) {
      out.push(rect);
    }
  }
  return out;
}

function parseNumberList(value: unknown): number[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.map((v) => Number(v)).filter((n) => Number.isInteger(n));
}

function parseConnectionState(value: unknown): ConnectionState {
  if (value === 'connected' || value === 'connecting' || value === 'disconnected') {
    return value;
  }
  return 'disconnected';
}

function parseHostStatus(value: unknown): HostStatusSnapshot | null {
  if (!isRecord(value)) {
    return null;
  }
  const selMonRaw = value['selected_monitor'];
  const selectedMonitor =
    typeof selMonRaw === 'number' && Number.isInteger(selMonRaw) ? selMonRaw : null;

  return {
    host_id: asString(value['host_id'], '---'),
    temp_password: asString(value['temp_password'], '---'),
    running: asBoolean(value['running'], false),
    phase: asString(value['phase'], 'idle'),
    backend_name: asString(value['backend_name'], 'Headless'),
    has_master_password: asBoolean(value['has_master_password'], false),
    viewer_name: asString(value['viewer_name'], ''),
    viewer_id: asString(value['viewer_id'], ''),
    e2e_enabled: asBoolean(value['e2e_enabled'], false),
    e2e_fingerprint: asString(value['e2e_fingerprint'], ''),
    e2e_cipher: asString(value['e2e_cipher'], 'X25519-AES256CTR-HMACSHA256'),
    monitors: parseMonitors(value['monitors']),
    selected_monitor: selectedMonitor,
    selected_monitors: parseNumberList(value['selected_monitors']),
  };
}

function parseSavedRemotes(value: unknown): SavedRemote[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const list: SavedRemote[] = [];
  for (const item of value) {
    if (isRecord(item) && typeof item['partner_id'] === 'string' && item['partner_id'].trim()) {
      list.push({
        partner_id: item['partner_id'],
        password: asString(item['password'], ''),
      });
    }
  }
  return list;
}

function parseFileTransfers(value: unknown): FileTransferItem[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const list: FileTransferItem[] = [];
  for (const item of value) {
    if (!isRecord(item)) {
      continue;
    }
    const dirRaw = asString(item['direction'], 'send').toLowerCase();
    const direction: TransferDirection = dirRaw === 'receive' ? 'receive' : 'send';
    const statusRaw = asString(item['status'], 'offered').toLowerCase();
    const savedPathRaw = item['saved_path'];
    const savedPath =
      typeof savedPathRaw === 'string' && savedPathRaw.trim().length > 0 ? savedPathRaw : null;
    list.push({
      file_id: asString(item['file_id'], ''),
      file_name: asString(item['file_name'], 'file'),
      file_size: asNumber(item['file_size'], 0),
      transferred_bytes: asNumber(item['transferred_bytes'], 0),
      direction,
      status: statusRaw,
      saved_path: savedPath,
    });
  }
  return list;
}

function parseUpdateState(value: unknown): UpdateState {
  if (
    value === 'idle' ||
    value === 'checking' ||
    value === 'available' ||
    value === 'up_to_date' ||
    value === 'downloading' ||
    value === 'ready' ||
    value === 'applying' ||
    value === 'applied' ||
    value === 'error'
  ) {
    return value;
  }
  return 'idle';
}

function parseUpdateStatus(value: unknown): UpdateStatusSnapshot | null {
  if (!isRecord(value)) {
    return null;
  }
  const pctRaw = value['download_progress_pct'];
  const pct = typeof pctRaw === 'number' && Number.isFinite(pctRaw) ? Math.round(pctRaw) : null;
  const totalRaw = value['total_bytes'];
  const total = typeof totalRaw === 'number' && Number.isFinite(totalRaw) ? totalRaw : null;
  const latestRaw = value['latest_version'];
  const latest =
    typeof latestRaw === 'string' && latestRaw.trim().length > 0 ? latestRaw.trim() : null;
  const notesRaw = value['release_notes'];
  const notes = typeof notesRaw === 'string' && notesRaw.trim().length > 0 ? notesRaw : null;
  const urlRaw = value['release_url'];
  const releaseUrl = typeof urlRaw === 'string' && urlRaw.trim().length > 0 ? urlRaw.trim() : null;
  const assetRaw = value['asset_name'];
  const assetName =
    typeof assetRaw === 'string' && assetRaw.trim().length > 0 ? assetRaw.trim() : null;
  const stagedRaw = value['staged_path'];
  const stagedPath =
    typeof stagedRaw === 'string' && stagedRaw.trim().length > 0 ? stagedRaw.trim() : null;
  const errRaw = value['error'];
  const error = typeof errRaw === 'string' && errRaw.trim().length > 0 ? errRaw.trim() : null;
  const dismissedRaw = value['dismissed_version'];
  const dismissed =
    typeof dismissedRaw === 'string' && dismissedRaw.trim().length > 0 ? dismissedRaw.trim() : null;
  const msgRaw = value['message'];
  const message = typeof msgRaw === 'string' && msgRaw.trim().length > 0 ? msgRaw.trim() : null;

  return {
    state: parseUpdateState(value['state']),
    current_version: asString(value['current_version'], '1.0.6'),
    latest_version: latest,
    release_notes: notes,
    release_url: releaseUrl,
    download_progress_pct: pct,
    downloaded_bytes: asNumber(value['downloaded_bytes'], 0),
    total_bytes: total,
    install_target: asString(value['install_target'], 'Unknown'),
    install_target_kind: asString(value['install_target_kind'], 'unknown'),
    asset_name: assetName,
    staged_path: stagedPath,
    error,
    dismissed_version: dismissed,
    message,
  };
}

export function parseSessionSnapshot(raw: unknown): SessionSnapshot {
  const obj = isRecord(raw) ? raw : {};
  const selMonRaw = obj['selected_monitor'];
  const selectedMonitor =
    typeof selMonRaw === 'number' && Number.isInteger(selMonRaw) ? selMonRaw : null;
  const cursorHashRaw = obj['cursor_hash'];
  const cursorHash =
    typeof cursorHashRaw === 'number' || typeof cursorHashRaw === 'string' ? cursorHashRaw : null;
  const errRaw = obj['error'];
  const error = typeof errRaw === 'string' && errRaw.trim().length > 0 ? errRaw : null;

  return {
    state: parseConnectionState(obj['state']),
    error,
    frames: asNumber(obj['frames'], 0),
    frame_version: asNumber(obj['frame_version'], asNumber(obj['frames'], 0)),
    partner_id: asString(obj['partner_id'], ''),
    saved_partner_id: asString(obj['saved_partner_id'], ''),
    saved_password: asString(obj['saved_password'], ''),
    remember_credentials: asBoolean(obj['remember_credentials'], true),
    saved_remotes: parseSavedRemotes(obj['saved_remotes']),
    saved_theme: asString(obj['saved_theme'], ''),
    saved_language: asString(obj['saved_language'], ''),
    saved_toolbar_collapsed: asBoolean(obj['saved_toolbar_collapsed'], false),
    e2e_enabled: asBoolean(obj['e2e_enabled'], false),
    e2e_fingerprint: asString(obj['e2e_fingerprint'], ''),
    e2e_cipher: asString(obj['e2e_cipher'], 'X25519-AES256CTR-HMACSHA256'),
    monitors: parseMonitors(obj['monitors']),
    selected_monitor: selectedMonitor,
    selected_monitors: parseNumberList(obj['selected_monitors']),
    cursor_png_base64: asString(obj['cursor_png_base64'], ''),
    cursor_hash: cursorHash,
    cursor_hot_x: Math.max(0, asNumber(obj['cursor_hot_x'], 0)),
    cursor_hot_y: Math.max(0, asNumber(obj['cursor_hot_y'], 0)),
    audio_enabled: asBoolean(obj['audio_enabled'], false),
    audio_chunks_received: asNumber(obj['audio_chunks_received'], 0),
    clipboard_local: asString(obj['clipboard_local'], ''),
    clipboard_remote: asString(obj['clipboard_remote'], ''),
    clipboard_updates_sent: asNumber(obj['clipboard_updates_sent'], 0),
    clipboard_updates_received: asNumber(obj['clipboard_updates_received'], 0),
    file_transfers: parseFileTransfers(obj['file_transfers']),
    chat_messages: Array.isArray(obj['chat_messages'])
      ? obj['chat_messages'].map((m) => String(m))
      : [],
    host_status: parseHostStatus(obj['host_status']),
    update_status: parseUpdateStatus(obj['update_status']),
  };
}
