export type ConnectionState = 'disconnected' | 'connecting' | 'connected';

export type MonitorRect = readonly [left: number, top: number, right: number, bottom: number];

export interface SavedRemote {
  readonly partner_id: string;
  readonly password?: string;
}

export type TransferDirection = 'send' | 'receive';
export type TransferStatus = 'offered' | 'transferring' | 'completed' | 'cancelled' | 'failed';

export interface FileTransferItem {
  readonly file_id: string;
  readonly file_name: string;
  readonly file_size: number;
  readonly transferred_bytes: number;
  readonly direction: TransferDirection;
  readonly status: TransferStatus | string;
  readonly saved_path: string | null;
}

export interface HostStatusSnapshot {
  readonly host_id: string;
  readonly temp_password: string;
  readonly running: boolean;
  readonly phase: string;
  readonly backend_name: string;
  readonly has_master_password: boolean;
  readonly viewer_name?: string;
  readonly viewer_id?: string;
  readonly e2e_enabled: boolean;
  readonly e2e_fingerprint: string;
  readonly e2e_cipher?: string;
  readonly monitors: readonly MonitorRect[];
  readonly selected_monitor: number | null;
  readonly selected_monitors: readonly number[];
}

export interface SessionSnapshot {
  readonly state: ConnectionState;
  readonly error: string | null;
  readonly frames: number;
  readonly frame_version: number;
  readonly partner_id: string;
  readonly saved_partner_id: string;
  readonly saved_password: string;
  readonly remember_credentials: boolean;
  readonly saved_remotes: readonly SavedRemote[];
  readonly saved_theme: string;
  readonly saved_language: string;
  readonly saved_toolbar_collapsed: boolean;
  readonly e2e_enabled: boolean;
  readonly e2e_fingerprint: string;
  readonly e2e_cipher: string;
  readonly monitors: readonly MonitorRect[];
  readonly selected_monitor: number | null;
  readonly selected_monitors: readonly number[];
  readonly cursor_png_base64: string;
  readonly cursor_hash: number | string | null;
  readonly cursor_hot_x: number;
  readonly cursor_hot_y: number;
  readonly audio_enabled: boolean;
  readonly audio_chunks_received: number;
  readonly clipboard_local: string;
  readonly clipboard_remote: string;
  readonly clipboard_updates_sent: number;
  readonly clipboard_updates_received: number;
  readonly file_transfers: readonly FileTransferItem[];
  readonly chat_messages: readonly string[];
  readonly host_status: HostStatusSnapshot | null;
}

export interface ActiveE2eInfo {
  readonly fingerprint: string;
  readonly cipher: string;
}

export function resolveActiveE2e(snapshot: SessionSnapshot | null): ActiveE2eInfo | null {
  if (!snapshot) {
    return null;
  }
  if (snapshot.state === 'connected' && snapshot.e2e_enabled && snapshot.e2e_fingerprint) {
    return {
      fingerprint: snapshot.e2e_fingerprint,
      cipher: snapshot.e2e_cipher || 'X25519-AES256CTR-HMACSHA256',
    };
  }
  const hs = snapshot.host_status;
  if (hs && hs.e2e_enabled && hs.e2e_fingerprint) {
    return {
      fingerprint: hs.e2e_fingerprint,
      cipher: hs.e2e_cipher || 'X25519-AES256CTR-HMACSHA256',
    };
  }
  return null;
}
