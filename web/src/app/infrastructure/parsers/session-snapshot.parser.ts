import {
  ConnectionState,
  FileTransferItem,
  HostStatusSnapshot,
  MonitorRect,
  SavedRemote,
  SessionSnapshot,
  TransferDirection,
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
    const dirRaw = item['direction'];
    const direction: TransferDirection = dirRaw === 'receive' ? 'receive' : 'send';
    list.push({
      file_id: asString(item['file_id'], ''),
      file_name: asString(item['file_name'], 'file'),
      file_size: asNumber(item['file_size'], 0),
      transferred_bytes: asNumber(item['transferred_bytes'], 0),
      direction,
      status: asString(item['status'], 'offered'),
    });
  }
  return list;
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
  };
}
