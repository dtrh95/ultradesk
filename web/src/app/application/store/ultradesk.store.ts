import { computed, Injectable, signal } from '@angular/core';
import {
  LanguageCode,
  LIGHT_PALETTES,
  NoticeBanner,
  OverlayKind,
  THEME_SHORT_LABELS,
  ThemeMode,
  ThemePalette,
  ToolTab,
} from '../../domain/models/preferences.model';
import {
  ConnectionState,
  FileTransferItem,
  resolveActiveE2e,
  SavedRemote,
  SessionSnapshot,
} from '../../domain/models/session.model';
import { I18nKey, translate } from '../../domain/services/i18n.catalog';
import { buildMonitorViewItems } from '../../domain/services/monitor-selection.domain-service';

@Injectable({ providedIn: 'root' })
export class UltraDeskStore {
  readonly snapshot = signal<SessionSnapshot | null>(null);
  readonly statusLost = signal<boolean>(false);
  readonly initiatingConnect = signal<boolean>(false);

  readonly overlay = signal<OverlayKind>('connect');
  readonly toolTab = signal<ToolTab>('chat');
  readonly lang = signal<LanguageCode>('en');
  readonly themeMode = signal<ThemeMode>('auto-time');
  readonly activeTheme = signal<ThemePalette>('onedark');
  readonly pillCollapsed = signal<boolean>(false);

  readonly notice = signal<NoticeBanner | null>(null);
  readonly copyConfirmed = signal<boolean>(false);
  readonly audioPlaying = signal<boolean>(true);

  readonly savedRemotes = signal<readonly SavedRemote[]>([]);
  readonly partnerId = signal<string>('');
  readonly partnerPass = signal<string>('');
  readonly rememberCreds = signal<boolean>(true);
  readonly clipInputText = signal<string>('');

  readonly themeShortLabel = computed(
    () => THEME_SHORT_LABELS[this.themeMode()] ?? this.themeMode(),
  );
  readonly isLightTheme = computed(() => LIGHT_PALETTES.includes(this.activeTheme()));

  readonly connectionState = computed<ConnectionState>(() => {
    if (this.statusLost()) {
      return 'disconnected';
    }
    if (this.initiatingConnect()) {
      return 'connecting';
    }
    const snap = this.snapshot();
    return snap ? snap.state : 'disconnected';
  });

  readonly statusText = computed<string>(() => {
    const lang = this.lang();
    if (this.statusLost()) {
      return translate(lang, 'status_lost');
    }
    if (this.initiatingConnect()) {
      return translate(lang, 'status_initiating');
    }
    const snap = this.snapshot();
    if (!snap) {
      return translate(lang, 'nav_connect');
    }
    const base =
      snap.state === 'connected'
        ? translate(lang, 'status_connected')
        : snap.state === 'connecting'
          ? translate(lang, 'status_connecting')
          : translate(lang, 'nav_connect');
    return base + (snap.error ? ` (${snap.error})` : '');
  });

  readonly activeE2e = computed(() => resolveActiveE2e(this.snapshot()));

  readonly e2eLabel = computed(() => {
    const info = this.activeE2e();
    return info ? translate(this.lang(), 'e2e_active', { fp: info.fingerprint }) : '';
  });

  readonly e2eTooltip = computed(() => {
    const info = this.activeE2e();
    return info
      ? translate(this.lang(), 'e2e_tooltip', { cipher: info.cipher, fp: info.fingerprint })
      : 'End-to-End Encrypted';
  });

  readonly remoteMonitorsView = computed(() => {
    const snap = this.snapshot();
    if (!snap) {
      return buildMonitorViewItems([], null, []);
    }
    return buildMonitorViewItems(snap.monitors, snap.selected_monitor, snap.selected_monitors);
  });

  readonly hostMonitorsView = computed(() => {
    const hs = this.snapshot()?.host_status;
    if (!hs) {
      return buildMonitorViewItems([], null, []);
    }
    return buildMonitorViewItems(hs.monitors, hs.selected_monitor, hs.selected_monitors);
  });

  readonly remoteMonitorSubsetOption = computed(() => {
    const view = this.remoteMonitorsView();
    if (view.allActive || view.selected.length <= 1) {
      return null;
    }
    const lang = this.lang();
    const listStr = view.selected
      .map((i) => translate(lang, 'monitor_short', { idx: i + 1 }))
      .join('+');
    return {
      value: `subset:${view.selected.join(',')}`,
      label: translate(lang, 'monitor_subset', { list: listStr }),
    };
  });

  readonly remoteMonitorSelectValue = computed(() => {
    const view = this.remoteMonitorsView();
    if (view.allActive) {
      return '';
    }
    if (view.selected.length === 1) {
      return String(view.selected[0]);
    }
    return `subset:${view.selected.join(',')}`;
  });

  readonly hostIdVal = computed(() => this.snapshot()?.host_status?.host_id || '---');
  readonly hostPassVal = computed(() => this.snapshot()?.host_status?.temp_password || '---');
  readonly hostPhaseBadge = computed(() => {
    const hs = this.snapshot()?.host_status;
    if (!hs) {
      return 'OFF: idle';
    }
    return `${hs.running ? 'ON: ' : 'OFF: '}${hs.phase}`;
  });
  readonly hostApprovalVisible = computed(() => this.snapshot()?.host_status?.phase === 'approval');
  readonly hostViewerPrompt = computed(() => {
    const hs = this.snapshot()?.host_status;
    return translate(this.lang(), 'host_viewer_prompt', {
      name: hs?.viewer_name ?? '',
      id: hs?.viewer_id ?? '',
    });
  });
  readonly hostStatusInfoText = computed(() => {
    const hs = this.snapshot()?.host_status;
    const lang = this.lang();
    const backend = hs?.backend_name || 'Headless';
    const mp = hs?.has_master_password ? translate(lang, 'mp_set') : translate(lang, 'mp_unset');
    const e2eSuffix = hs?.e2e_enabled && hs.e2e_fingerprint ? ` | E2E ${hs.e2e_fingerprint}` : '';
    return translate(lang, 'host_status_info', { backend, mp }) + e2eSuffix;
  });

  readonly macosPermissions = computed(
    () => this.snapshot()?.host_status?.macos_permissions ?? null,
  );
  readonly showMacosPermissionBox = computed(() => {
    const p = this.macosPermissions();
    if (!p) {
      return false;
    }
    return p.screen_recording === 'not_granted' || p.accessibility === 'not_granted';
  });

  readonly audioStatsText = computed(() => `${this.snapshot()?.audio_chunks_received ?? 0} chunks`);
  readonly audioStatusText = computed(() => {
    const snap = this.snapshot();
    return snap?.audio_enabled
      ? translate(this.lang(), 'audio_on')
      : translate(this.lang(), 'audio_off');
  });
  readonly clipStatsText = computed(() => {
    const snap = this.snapshot();
    return `${snap?.clipboard_updates_sent ?? 0}↑ / ${snap?.clipboard_updates_received ?? 0}↓`;
  });
  readonly remoteClipboardText = computed(() => this.snapshot()?.clipboard_remote ?? '');
  readonly fileTransfers = computed<readonly FileTransferItem[]>(
    () => this.snapshot()?.file_transfers ?? [],
  );
  readonly chatMessages = computed<readonly string[]>(() => this.snapshot()?.chat_messages ?? []);
  readonly updateStatus = computed(() => this.snapshot()?.update_status ?? null);
  readonly updateBadgeVisible = computed(() => {
    const u = this.updateStatus();
    return Boolean(
      u &&
      (u.state === 'available' ||
        u.state === 'downloading' ||
        u.state === 'applying' ||
        u.state === 'ready' ||
        u.state === 'applied') &&
      u.latest_version,
    );
  });
  readonly shouldAutoOpenUpdateModal = computed(() => {
    const u = this.updateStatus();
    return Boolean(
      u && u.state === 'available' && u.latest_version && u.dismissed_version !== u.latest_version,
    );
  });

  t(key: I18nKey, vars: Readonly<Record<string, string | number>> = {}): string {
    return translate(this.lang(), key, vars);
  }
}
