import { inject, Injectable } from '@angular/core';
import { MouseButton, RemoteInputEvent } from '../../domain/models/input.model';
import {
  LanguageCode,
  NoticeKind,
  OverlayKind,
  ThemeMode,
  ToolTab,
} from '../../domain/models/preferences.model';
import { SavedRemote } from '../../domain/models/session.model';
import {
  computeNextCycledMonitorSelection,
  normalizeSelectedIndices,
  toggleMonitorSelection,
} from '../../domain/services/monitor-selection.domain-service';
import {
  computeNextThemeMode,
  isValidThemeMode,
  resolveActiveTheme,
} from '../../domain/services/theme-resolver.domain-service';
import { WebAudioPlayerAdapter } from '../../infrastructure/audio/web-audio-player.adapter';
import { PortalHttpAdapter } from '../../infrastructure/http/portal-http.adapter';
import { NativeIpcAdapter } from '../../infrastructure/ipc/native-ipc.adapter';
import { LocalPreferencesStorage } from '../../infrastructure/storage/local-preferences.storage';
import { UltraDeskStore } from '../store/ultradesk.store';

@Injectable({ providedIn: 'root' })
export class UltraDeskSessionFacade {
  readonly store = inject(UltraDeskStore);
  private readonly http = inject(PortalHttpAdapter);
  private readonly ipc = inject(NativeIpcAdapter);
  private readonly storage = inject(LocalPreferencesStorage);
  private readonly audioPlayer = inject(WebAudioPlayerAdapter);

  private noticeTimer: ReturnType<typeof setTimeout> | null = null;
  private copyTimer: ReturnType<typeof setTimeout> | null = null;
  private statusPollBusy = false;
  private prefsHydrated = false;
  private serverPrefsApplied = false;
  private credsInitialized = false;
  private initialConnectCheckDone = false;
  private lastApprovalAutoOpened = false;
  private autoOpenedUpdateVersion: string | null = null;

  private inputChain: Promise<void> = Promise.resolve();
  private readonly heldKeys = new Set<number>();
  private readonly heldButtons = new Set<MouseButton>();
  private lastUserActivity = performance.now();
  private staticRefreshed = false;
  private frameInFlight = false;
  private lastFrameVersion = 0;
  private lastRenderedQuality = 0;
  private frameToggle = false;
  private lastCursorHash: number | string | null = null;

  private canvasEl: HTMLCanvasElement | null = null;
  private canvasCtx: CanvasRenderingContext2D | null = null;
  private onPopoverLayoutHook: (() => void) | null = null;

  initialize(): void {
    this.ipc.registerGlobalBridge({
      onDispatchAction: (action, payload) => this.handleHostDispatchedAction(action, payload),
      onSetPillCollapsed: (collapsed, persist = false) => this.setPillCollapsed(collapsed, persist),
      onShowNotice: (msg, kind = 'error', durationMs = 4200) =>
        this.showNotice(msg, kind, durationMs),
      onRemoteMonClick: (idx, event) => this.handleRemoteMonitorClick(idx, event),
      onRemoteMonCheck: (idx, event) => this.handleRemoteMonitorCheck(idx, event),
      onHostMonClick: (idx, event) => this.handleHostMonitorClick(idx, event),
      onHostMonCheck: (idx, event) => this.handleHostMonitorCheck(idx, event),
      onAcceptTransfer: (fileId) => void this.acceptTransfer(fileId),
      onCancelTransfer: (fileId) => void this.cancelTransfer(fileId),
    });

    const hydrated = this.storage.hydrateFromUrlAndStorage();
    this.store.lang.set(hydrated.lang);
    this.store.themeMode.set(hydrated.themeMode);
    this.setTheme(hydrated.themeMode, false, hydrated.hourOverride);
    this.store.pillCollapsed.set(hydrated.pillCollapsed);
    if (hydrated.savedRemotes.length > 0) {
      this.store.savedRemotes.set(hydrated.savedRemotes);
    }
    if (hydrated.savedCreds) {
      this.store.rememberCreds.set(hydrated.savedCreds.remember);
      if (hydrated.savedCreds.remember) {
        if (hydrated.savedCreds.id) {
          this.store.partnerId.set(hydrated.savedCreds.id);
        }
        if (hydrated.savedCreds.pass) {
          this.store.partnerPass.set(hydrated.savedCreds.pass);
        }
      }
    }

    this.setLanguage(this.store.lang(), false);
    this.setPillCollapsed(this.store.pillCollapsed(), false);
    this.prefsHydrated = true;

    if (typeof window.matchMedia === 'function') {
      try {
        window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => {
          if (this.store.themeMode() === 'auto-os') {
            this.setTheme('auto-os', false);
          }
        });
      } catch {
        void 0;
      }
    }

    setInterval(() => {
      if (this.store.themeMode() === 'auto-time') {
        const nextActive = resolveActiveTheme('auto-time', null, this.osPrefersLight());
        if (nextActive !== this.store.activeTheme()) {
          this.setTheme('auto-time', false);
        }
      }
    }, 30000);

    setInterval(() => {
      void this.refreshStatus();
    }, 120);
    void this.refresh();
    this.startFrameRafLoop();
  }

  registerCanvas(canvas: HTMLCanvasElement): void {
    this.canvasEl = canvas;
    this.canvasCtx = canvas.getContext('2d');
    this.drawIdleCanvas();
  }

  registerPopoverLayoutHook(fn: () => void): void {
    this.onPopoverLayoutHook = fn;
  }

  focusScreen(): void {
    this.canvasEl?.focus();
  }

  hideNotice(): void {
    this.store.notice.set(null);
    if (this.noticeTimer) {
      clearTimeout(this.noticeTimer);
      this.noticeTimer = null;
    }
  }

  showNotice(msg: unknown, kind: NoticeKind = 'error', durationMs = 4200): void {
    if (this.noticeTimer) {
      clearTimeout(this.noticeTimer);
      this.noticeTimer = null;
    }
    const cleanMsg =
      typeof msg === 'string'
        ? msg
        : typeof msg === 'object' && msg !== null && 'message' in msg
          ? String((msg as { message: unknown }).message)
          : String(msg);
    this.store.notice.set({ message: cleanMsg, kind });
    if (durationMs > 0) {
      this.noticeTimer = setTimeout(() => this.hideNotice(), durationMs);
    }
  }

  setOverlay(next: OverlayKind): void {
    this.store.overlay.set(next);
    if (this.onPopoverLayoutHook) {
      setTimeout(() => this.onPopoverLayoutHook?.(), 0);
    }
    if (next === 'none') {
      this.focusScreen();
    }
  }

  setToolTab(tab: ToolTab): void {
    this.store.toolTab.set(tab);
    if (this.onPopoverLayoutHook) {
      setTimeout(() => this.onPopoverLayoutHook?.(), 0);
    }
  }

  setPillCollapsed(collapsed: boolean, persist = false): void {
    const next = Boolean(collapsed);
    this.store.pillCollapsed.set(next);
    this.storage.savePillCollapsed(next);
    if (persist && this.prefsHydrated) {
      void this.http
        .postJson('/preferences', { toolbar_collapsed: next }, this.store.t('err_failed'))
        .catch(() => undefined);
    }
    if (next) {
      if (this.store.overlay() !== 'none') {
        this.setOverlay('none');
      }
      this.focusScreen();
    }
    this.ipc.notifyHostIfNotDispatching('SetToolbarCollapsed', next);
  }

  setTheme(mode: string, persist = true, hourOverride: number | null = null): void {
    const cleanMode: ThemeMode = isValidThemeMode(mode) ? mode : 'auto-time';
    const active = resolveActiveTheme(cleanMode, hourOverride, this.osPrefersLight());
    this.store.themeMode.set(cleanMode);
    this.store.activeTheme.set(active);
    document.documentElement.setAttribute('data-theme', active);
    document.documentElement.setAttribute('data-theme-mode', cleanMode);

    if (persist) {
      this.storage.saveThemeMode(cleanMode);
      if (this.prefsHydrated) {
        void this.http
          .postJson('/preferences', { theme: cleanMode }, this.store.t('err_failed'))
          .catch(() => undefined);
      }
    }
    this.ipc.notifyHostIfNotDispatching('SetTheme', cleanMode);
    this.drawIdleCanvas();
  }

  cycleTheme(shiftKey = false): void {
    const nextMode = computeNextThemeMode(
      this.store.themeMode(),
      this.store.activeTheme(),
      shiftKey,
    );
    this.setTheme(nextMode, true);
  }

  setLanguage(lang: string, persist = false): void {
    const cleanLang: LanguageCode = lang === 'en' ? 'en' : 'vi';
    this.store.lang.set(cleanLang);
    this.storage.saveLanguage(cleanLang);
    if (persist && this.prefsHydrated) {
      void this.http
        .postJson('/preferences', { language: cleanLang }, this.store.t('err_failed'))
        .catch(() => undefined);
    }
    document.documentElement.lang = cleanLang;
    this.drawIdleCanvas();
    this.ipc.notifyHostIfNotDispatching('SetLanguage', cleanLang);
  }

  toggleFullscreen(): void {
    if (!this.ipc.isDispatchingFromHost() && this.ipc.hasNativeIpc()) {
      this.ipc.postMessage({ action: 'ToggleFullscreen' });
    } else if (!this.ipc.hasNativeIpc()) {
      if (!document.fullscreenElement) {
        void document.documentElement.requestFullscreen().catch(() => undefined);
      } else {
        void document.exitFullscreen().catch(() => undefined);
      }
    }
  }

  async copyHostId(): Promise<void> {
    const idText = this.store.hostIdVal().trim();
    if (!idText || idText === '---') {
      return;
    }
    try {
      await navigator.clipboard.writeText(idText);
      if (this.copyTimer) {
        clearTimeout(this.copyTimer);
      }
      this.store.copyConfirmed.set(true);
      this.copyTimer = setTimeout(() => {
        this.store.copyConfirmed.set(false);
        this.copyTimer = null;
      }, 1500);
    } catch {
      void 0;
    }
  }

  selectSavedRemote(partnerId: string): void {
    if (!partnerId) {
      return;
    }
    const found = this.store.savedRemotes().find((r) => r.partner_id === partnerId);
    if (found) {
      this.store.partnerId.set(found.partner_id);
      this.store.partnerPass.set(found.password ?? '');
    }
  }

  updateRememberCreds(remember: boolean): void {
    this.store.rememberCreds.set(remember);
    const id = this.store.partnerId().trim();
    const pass = this.store.partnerPass().trim();
    this.storage.saveCredentials(id, pass, remember);
    void this.http
      .postJson('/credentials', { id, pass, remember }, this.store.t('err_failed'))
      .catch(() => undefined);
  }

  async connectRemote(): Promise<void> {
    const id = this.store.partnerId().trim();
    const pass = this.store.partnerPass().trim();
    const remember = this.store.rememberCreds();
    if (!id || !pass) {
      this.showNotice(this.store.t('err_missing_creds'), 'error');
      return;
    }
    this.storage.saveCredentials(id, pass, remember);
    this.store.initiatingConnect.set(true);
    try {
      await this.http.postJson('/connect', { id, pass, remember }, this.store.t('err_failed'));
      this.setOverlay('none');
    } catch (e: unknown) {
      this.store.initiatingConnect.set(false);
      this.showNotice(e, 'error');
    }
  }

  disconnectRemote(): void {
    this.releaseAllInputs();
    void this.inputChain
      .then(() => this.http.postJson('/disconnect', {}, this.store.t('err_failed')))
      .catch(() => undefined);
  }

  sendSuperKey(): void {
    this.focusScreen();
    this.sendInput({ type: 'key', vk: 91, down: true });
    setTimeout(() => this.sendInput({ type: 'key', vk: 91, down: false }), 60);
  }

  sendCtrlAltDel(): void {
    this.focusScreen();
    this.sendInput({ type: 'key', vk: 17, down: true });
    this.sendInput({ type: 'key', vk: 18, down: true });
    this.sendInput({ type: 'key', vk: 46, down: true });
    setTimeout(() => {
      this.sendInput({ type: 'key', vk: 46, down: false });
      this.sendInput({ type: 'key', vk: 18, down: false });
      this.sendInput({ type: 'key', vk: 17, down: false });
    }, 60);
  }

  sendInput(event: RemoteInputEvent): void {
    this.lastUserActivity = performance.now();
    this.staticRefreshed = false;
    if (
      this.ipc.hasNativeIpc() &&
      this.ipc.postMessage({ action: 'input', payload: event, event })
    ) {
      return;
    }
    const fallbackErr = this.store.t('err_failed');
    this.inputChain = this.inputChain
      .then(() => this.http.sendInputEvent(event, fallbackErr))
      .catch(() => undefined);
  }

  sendMoveEvent(x: number, y: number, onDone: () => void): boolean {
    this.lastUserActivity = performance.now();
    this.staticRefreshed = false;
    const evt: RemoteInputEvent = { type: 'move', x, y };
    if (
      this.ipc.hasNativeIpc() &&
      this.ipc.postMessage({ action: 'input', payload: evt, event: evt })
    ) {
      return true;
    }
    void this.http
      .sendInputEvent(evt, this.store.t('err_failed'))
      .catch(() => undefined)
      .finally(onDone);
    return false;
  }

  trackButtonDown(button: MouseButton): void {
    this.heldButtons.add(button);
  }

  trackButtonUp(button: MouseButton): void {
    this.heldButtons.delete(button);
  }

  isKeyHeld(vk: number): boolean {
    return this.heldKeys.has(vk);
  }

  hasAnyHeldKeys(): boolean {
    return this.heldKeys.size > 0;
  }

  pressKey(vk: number): void {
    this.heldKeys.add(vk);
    this.sendInput({ type: 'key', vk, down: true });
  }

  releaseKey(vk: number): void {
    this.heldKeys.delete(vk);
    this.sendInput({ type: 'key', vk, down: false });
  }

  releaseAllInputs(): void {
    for (const vk of this.heldKeys) {
      this.sendInput({ type: 'key', vk, down: false });
    }
    this.heldKeys.clear();
    for (const button of this.heldButtons) {
      this.sendInput({ type: 'button', button, down: false });
    }
    this.heldButtons.clear();
  }

  selectRemoteMonitors(indices: readonly number[] | null): void {
    const payload = Array.isArray(indices) ? { indices } : { index: null, indices: null };
    void this.http.postJson('/monitor', payload, this.store.t('err_failed')).catch(() => undefined);
  }

  selectHostMonitors(indices: readonly number[] | null): void {
    const payload = Array.isArray(indices) ? { indices } : { index: null, indices: null };
    void this.http
      .postJson('/host/monitor', payload, this.store.t('err_failed'))
      .catch(() => undefined);
  }

  cycleRemoteMonitor(): void {
    const snap = this.store.snapshot();
    if (!snap) {
      return;
    }
    const next = computeNextCycledMonitorSelection(
      snap.monitors,
      snap.selected_monitor,
      snap.selected_monitors,
    );
    if (next !== undefined) {
      this.selectRemoteMonitors(next);
    }
  }

  handleRemoteMonitorClick(idx: number, event?: MouseEvent): void {
    event?.stopPropagation();
    const snap = this.store.snapshot();
    if (!snap || snap.monitors.length === 0) {
      return;
    }
    if (idx < 0) {
      this.selectRemoteMonitors(null);
      return;
    }
    const sel = normalizeSelectedIndices(
      snap.monitors,
      snap.selected_monitor,
      snap.selected_monitors,
    );
    const forceToggle = Boolean(event && (event.shiftKey || event.ctrlKey || event.metaKey));
    const next = toggleMonitorSelection(snap.monitors.length, sel, idx, forceToggle);
    this.selectRemoteMonitors(next);
  }

  handleRemoteMonitorCheck(idx: number, event?: MouseEvent): void {
    event?.stopPropagation();
    const snap = this.store.snapshot();
    if (!snap || snap.monitors.length === 0) {
      return;
    }
    const sel = normalizeSelectedIndices(
      snap.monitors,
      snap.selected_monitor,
      snap.selected_monitors,
    );
    const next = toggleMonitorSelection(snap.monitors.length, sel, idx, true);
    this.selectRemoteMonitors(next);
  }

  handleHostMonitorClick(idx: number, event?: MouseEvent): void {
    event?.stopPropagation();
    const hs = this.store.snapshot()?.host_status;
    if (!hs || hs.monitors.length === 0) {
      return;
    }
    if (idx < 0) {
      this.selectHostMonitors(null);
      return;
    }
    const sel = normalizeSelectedIndices(hs.monitors, hs.selected_monitor, hs.selected_monitors);
    const forceToggle = Boolean(event && (event.shiftKey || event.ctrlKey || event.metaKey));
    const next = toggleMonitorSelection(hs.monitors.length, sel, idx, forceToggle);
    this.selectHostMonitors(next);
  }

  handleHostMonitorCheck(idx: number, event?: MouseEvent): void {
    event?.stopPropagation();
    const hs = this.store.snapshot()?.host_status;
    if (!hs || hs.monitors.length === 0) {
      return;
    }
    const sel = normalizeSelectedIndices(hs.monitors, hs.selected_monitor, hs.selected_monitors);
    const next = toggleMonitorSelection(hs.monitors.length, sel, idx, true);
    this.selectHostMonitors(next);
  }

  handleMonitorDropdownChange(val: string): void {
    if (val === '') {
      this.selectRemoteMonitors(null);
    } else if (val.startsWith('subset:')) {
      const indices = val
        .slice(7)
        .split(',')
        .map(Number)
        .filter((n) => Number.isInteger(n));
      this.selectRemoteMonitors(indices);
    } else {
      this.selectRemoteMonitors([Number(val)]);
    }
  }

  startHost(): void {
    void this.http
      .postJson('/host/start', {}, this.store.t('err_failed'))
      .catch((e: unknown) => this.showNotice(e, 'error'));
  }

  stopHost(): void {
    void this.http
      .postJson('/host/stop', {}, this.store.t('err_failed'))
      .catch((e: unknown) => this.showNotice(e, 'error'));
  }

  approveHost(allowControl: boolean): void {
    void this.http
      .postJson('/host/approve', { allow_control: allowControl }, this.store.t('err_failed'))
      .catch((e: unknown) => this.showNotice(e, 'error'));
  }

  declineHost(): void {
    void this.http
      .postJson('/host/decline', {}, this.store.t('err_failed'))
      .catch((e: unknown) => this.showNotice(e, 'error'));
  }

  async saveMasterPassword(password: string): Promise<void> {
    const pw = password.trim();
    try {
      await this.http.postJson(
        '/host/master-password',
        { password: pw || null },
        this.store.t('err_failed'),
      );
    } catch (e: unknown) {
      this.showNotice(e, 'error');
    }
  }

  async enableAudio(): Promise<void> {
    this.store.audioPlaying.set(true);
    await this.audioPlayer.ensureResumed();
    await this.http
      .postJson('/audio/toggle', { enable: true }, this.store.t('err_failed'))
      .catch(() => undefined);
  }

  async muteAudio(): Promise<void> {
    this.store.audioPlaying.set(false);
    await this.http
      .postJson('/audio/toggle', { enable: false }, this.store.t('err_failed'))
      .catch(() => undefined);
  }

  async sendClipboardToPartner(text: string): Promise<void> {
    try {
      await this.http.postJson(
        '/clipboard',
        { text, sync_from_system: false },
        this.store.t('err_failed'),
      );
    } catch (e: unknown) {
      this.showNotice(e, 'error');
    }
  }

  async pullLinuxClipboard(): Promise<void> {
    try {
      const res = await this.http.postJson(
        '/clipboard',
        { text: '', sync_from_system: true },
        this.store.t('err_failed'),
      );
      if (typeof res['text'] === 'string') {
        this.store.clipInputText.set(res['text']);
      }
    } catch {
      void 0;
    }
  }

  async sendUnicodeText(text: string): Promise<void> {
    if (!text) {
      return;
    }
    try {
      await this.http.postJson('/text', { text, caps_lock: false }, this.store.t('err_failed'));
    } catch (e: unknown) {
      this.showNotice(e, 'error');
    }
  }

  async sendSelectedFile(file: File | undefined): Promise<void> {
    if (!file) {
      this.showNotice(this.store.t('err_missing_file'), 'error');
      return;
    }
    const buf = await file.arrayBuffer();
    const bytes = new Uint8Array(buf);
    let bin = '';
    for (const byte of bytes) {
      bin += String.fromCharCode(byte);
    }
    try {
      await this.http.postJson(
        '/files/send',
        { file_name: file.name, data_base64: btoa(bin) },
        this.store.t('err_failed'),
      );
    } catch (e: unknown) {
      this.showNotice(e, 'error');
    }
  }

  async sendFileByPath(filePath: string): Promise<void> {
    const clean = filePath.trim();
    if (!clean) {
      return;
    }
    try {
      await this.http.postJson('/files/send', { file_path: clean }, this.store.t('err_failed'));
    } catch (e: unknown) {
      this.showNotice(e, 'error');
    }
  }

  async receiveRemoteFileByPath(remotePath: string): Promise<void> {
    const clean = remotePath.trim();
    if (!clean) {
      return;
    }
    try {
      await this.http.postJson(
        '/files/receive',
        { remote_path: clean },
        this.store.t('err_failed'),
      );
    } catch (e: unknown) {
      this.showNotice(e, 'error');
    }
  }

  async acceptTransfer(fileId: string): Promise<void> {
    await this.http
      .postJson('/files/accept', { file_id: fileId }, this.store.t('err_failed'))
      .catch(() => undefined);
  }

  async cancelTransfer(fileId: string): Promise<void> {
    await this.http
      .postJson('/files/cancel', { file_id: fileId }, this.store.t('err_failed'))
      .catch(() => undefined);
  }

  openUpdateModal(): void {
    this.setOverlay('update');
  }

  async checkForUpdates(apiUrl?: string): Promise<void> {
    try {
      await this.http.checkUpdate(apiUrl);
      await this.refreshStatus();
    } catch (e: unknown) {
      this.showNotice(e, 'error');
    }
  }

  async confirmUpdate(): Promise<void> {
    try {
      const res = await this.http.confirmUpdate();
      await this.refreshStatus();
      if (typeof res['message'] === 'string' && res['message']) {
        this.showNotice(res['message'], 'info');
      }
    } catch (e: unknown) {
      await this.refreshStatus();
      this.showNotice(e, 'error');
    }
  }

  async dismissUpdate(version?: string | null): Promise<void> {
    const nextOverlay = this.store.snapshot()?.state === 'connected' ? 'none' : 'connect';
    try {
      const targetVer = version || this.store.updateStatus()?.latest_version || null;
      await this.http.dismissUpdate(targetVer);
      this.setOverlay(nextOverlay);
      await this.refreshStatus();
    } catch {
      this.setOverlay(nextOverlay);
    }
  }

  async sendChatMessage(text: string): Promise<void> {
    const clean = text.trim();
    if (!clean) {
      return;
    }
    try {
      await this.http.postJson('/chat', { text: clean }, this.store.t('err_failed'));
    } catch (e: unknown) {
      this.showNotice(e, 'error');
    }
  }

  drawIdleCanvas(): void {
    const snap = this.store.snapshot();
    if (snap && snap.frames > 0) {
      return;
    }
    const screen = this.canvasEl;
    const ctx = this.canvasCtx;
    if (!screen || !ctx) {
      return;
    }
    const w = screen.width;
    const h = screen.height;
    ctx.fillStyle = this.cssVar('--stage-bg', '#141617');
    ctx.fillRect(0, 0, w, h);

    const step = 144;
    ctx.strokeStyle = this.cssVar('--line', 'rgba(255,255,255,0.08)');
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let x = step; x < w; x += step) {
      for (let y = step; y < h; y += step) {
        ctx.moveTo(x - 4, y);
        ctx.lineTo(x + 4, y);
        ctx.moveTo(x, y - 4);
        ctx.lineTo(x, y + 4);
      }
    }
    ctx.stroke();

    const cx = Math.round(w / 2);
    const cy = Math.round(h / 2 - 20);
    const r = 34;
    ctx.strokeStyle = this.cssVar('--line-strong', 'rgba(255,255,255,0.22)');
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.stroke();

    ctx.strokeStyle = this.cssVar('--line', 'rgba(255,255,255,0.12)');
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(cx - r - 8, cy);
    ctx.lineTo(cx - r + 4, cy);
    ctx.moveTo(cx + r - 4, cy);
    ctx.lineTo(cx + r + 8, cy);
    ctx.moveTo(cx, cy - r - 8);
    ctx.lineTo(cx, cy - r + 4);
    ctx.moveTo(cx, cy + r - 4);
    ctx.lineTo(cx, cy + r + 8);
    ctx.stroke();

    ctx.strokeStyle = this.cssVar('--action', '#61afef');
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    for (let i = 0; i < 6; i++) {
      const a1 = (i * Math.PI) / 3;
      const a2 = ((i + 2) * Math.PI) / 3;
      const p1x = cx + r * Math.cos(a1);
      const p1y = cy + r * Math.sin(a1);
      const p2x = cx + r * 0.55 * Math.cos(a2);
      const p2y = cy + r * 0.55 * Math.sin(a2);
      ctx.moveTo(p1x, p1y);
      ctx.lineTo(p2x, p2y);
    }
    ctx.stroke();

    ctx.fillStyle = this.cssVar('--surface-variant', 'rgba(255,255,255,0.06)');
    ctx.strokeStyle = this.cssVar('--line-strong', 'rgba(255,255,255,0.18)');
    ctx.lineWidth = 1;
    const pillW = 240;
    const pillH = 24;
    const pillX = Math.round(cx - pillW / 2);
    const pillY = cy + r + 22;
    ctx.beginPath();
    const rad = 4;
    ctx.moveTo(pillX + rad, pillY);
    ctx.lineTo(pillX + pillW - rad, pillY);
    ctx.quadraticCurveTo(pillX + pillW, pillY, pillX + pillW, pillY + rad);
    ctx.lineTo(pillX + pillW, pillY + pillH - rad);
    ctx.quadraticCurveTo(pillX + pillW, pillY + pillH, pillX + pillW - rad, pillY + pillH);
    ctx.lineTo(pillX + rad, pillY + pillH);
    ctx.quadraticCurveTo(pillX, pillY + pillH, pillX, pillY + pillH - rad);
    ctx.lineTo(pillX, pillY + rad);
    ctx.quadraticCurveTo(pillX, pillY, pillX + rad, pillY);
    ctx.closePath();
    ctx.fill();
    ctx.stroke();

    ctx.fillStyle = this.cssVar('--muted', 'rgba(255,255,255,0.7)');
    ctx.font = `500 12px ${this.cssVar('--mono', 'monospace')}`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText('1280 × 720  ·  X25519 / RGB555', cx, pillY + pillH / 2);

    ctx.fillStyle = this.cssVar('--muted', 'rgba(255,255,255,0.7)');
    ctx.font = `500 14px ${this.cssVar('--sans', 'sans-serif')}`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'alphabetic';
    ctx.fillText(this.store.t('idle_hint'), cx, pillY + pillH + 28);
  }

  cssVar(name: string, fallback: string): string {
    const val = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return val || fallback;
  }

  private osPrefersLight(): boolean {
    return Boolean(
      typeof window.matchMedia === 'function' &&
      window.matchMedia('(prefers-color-scheme: light)').matches,
    );
  }

  private updateSavedRemotesCache(remotes: readonly SavedRemote[]): void {
    const prevSig = JSON.stringify(this.store.savedRemotes());
    const nextSig = JSON.stringify(remotes);
    if (prevSig === nextSig) {
      return;
    }
    this.store.savedRemotes.set(remotes);
    this.storage.saveRemotesList(remotes);
  }

  private async refreshStatus(): Promise<void> {
    if (this.statusPollBusy) {
      return;
    }
    this.statusPollBusy = true;
    try {
      const s = await this.http.fetchStatus();
      this.store.statusLost.set(false);
      this.store.initiatingConnect.set(false);
      this.store.snapshot.set(s);

      if (!this.serverPrefsApplied) {
        this.serverPrefsApplied = true;
        const urlParams = new URLSearchParams(window.location.search);
        if (
          !urlParams.get('theme') &&
          s.saved_theme &&
          isValidThemeMode(s.saved_theme) &&
          s.saved_theme !== this.store.themeMode()
        ) {
          this.setTheme(s.saved_theme, false);
        }
        if (
          !urlParams.get('lang') &&
          (s.saved_language === 'en' || s.saved_language === 'vi') &&
          s.saved_language !== this.store.lang()
        ) {
          this.setLanguage(s.saved_language, false);
        }
      }

      if (s.saved_remotes.length > 0 || this.store.savedRemotes().length > 0) {
        this.updateSavedRemotesCache(s.saved_remotes);
      }

      if (!this.credsInitialized) {
        this.credsInitialized = true;
        this.store.rememberCreds.set(s.remember_credentials);
        if (!this.store.partnerId() && (s.saved_partner_id || s.partner_id)) {
          this.store.partnerId.set(s.saved_partner_id || s.partner_id);
        }
        if (!this.store.partnerPass() && s.saved_password) {
          this.store.partnerPass.set(s.saved_password);
        }
        if (!this.store.partnerId() && s.saved_remotes.length > 0) {
          this.store.partnerId.set(s.saved_remotes[0].partner_id);
          if (!this.store.partnerPass()) {
            this.store.partnerPass.set(s.saved_remotes[0].password ?? '');
          }
        }
        if (this.store.partnerId() || this.store.partnerPass()) {
          this.storage.saveCredentials(
            this.store.partnerId(),
            this.store.partnerPass(),
            this.store.rememberCreds(),
          );
        }
      }

      if (!this.initialConnectCheckDone) {
        this.initialConnectCheckDone = true;
        if (s.state === 'connected' && s.frames > 0 && this.store.overlay() === 'connect') {
          this.setOverlay('none');
        }
      }

      const u = s.update_status;
      const hs = s.host_status;
      if (
        u &&
        u.state === 'available' &&
        u.latest_version &&
        u.dismissed_version !== u.latest_version &&
        this.autoOpenedUpdateVersion !== u.latest_version &&
        s.state !== 'connected' &&
        !s.frames &&
        (this.store.overlay() === 'none' ||
          (this.store.overlay() === 'connect' && hs?.phase !== 'approval'))
      ) {
        this.autoOpenedUpdateVersion = u.latest_version;
        this.setOverlay('update');
      }

      if (hs?.phase === 'approval') {
        if (!this.lastApprovalAutoOpened) {
          this.lastApprovalAutoOpened = true;
          this.setOverlay('connect');
        }
      } else {
        this.lastApprovalAutoOpened = false;
      }

      this.syncCanvasCursor(
        s.frames,
        s.cursor_png_base64,
        s.cursor_hash,
        s.cursor_hot_x,
        s.cursor_hot_y,
      );
      await this.audioPlayer.pollAndPlayAudio(s.audio_chunks_received, this.store.audioPlaying());

      if (!s.frames) {
        this.drawIdleCanvas();
      }
    } catch {
      this.store.initiatingConnect.set(false);
      this.store.statusLost.set(true);
    } finally {
      this.statusPollBusy = false;
    }
  }

  private syncCanvasCursor(
    frames: number,
    cursorPngBase64: string,
    cursorHash: number | string | null,
    hotX: number,
    hotY: number,
  ): void {
    const screen = this.canvasEl;
    if (!screen) {
      return;
    }
    if (frames > 0 && cursorPngBase64) {
      if (cursorHash !== this.lastCursorHash) {
        this.lastCursorHash = cursorHash;
        screen.style.cursor = `url("data:image/png;base64,${cursorPngBase64}") ${hotX} ${hotY}, default`;
      }
    } else if (this.lastCursorHash !== null) {
      this.lastCursorHash = null;
      screen.style.cursor = 'default';
    }
  }

  private async fetchAndRenderFrame(): Promise<void> {
    const snap = this.store.snapshot();
    const screen = this.canvasEl;
    const ctx = this.canvasCtx;
    if (this.frameInFlight || !snap || !snap.frames || !screen || !ctx) {
      return;
    }
    const fVer = snap.frame_version || snap.frames || 0;
    const isInteracting = performance.now() - this.lastUserActivity < 200;
    const targetQuality = isInteracting ? 72 : 92;
    const needNewVersion = fVer !== this.lastFrameVersion;
    const needStaticRefresh =
      !isInteracting && !this.staticRefreshed && this.lastRenderedQuality < 90;
    if (!needNewVersion && !needStaticRefresh) {
      return;
    }

    this.frameInFlight = true;
    try {
      const res = await this.http.fetchFrame(
        needNewVersion,
        this.lastFrameVersion,
        targetQuality,
        fVer,
      );
      if (!res) {
        return;
      }
      if (res.kind === 'not-modified') {
        this.lastFrameVersion = fVer;
        return;
      }
      if (res.kind === 'frame' && res.blob) {
        const objUrl = URL.createObjectURL(res.blob);
        try {
          const img = new Image();
          img.src = objUrl;
          await img.decode();
          if (screen.width !== img.naturalWidth || screen.height !== img.naturalHeight) {
            screen.width = img.naturalWidth;
            screen.height = img.naturalHeight;
          }
          ctx.clearRect(0, 0, screen.width, screen.height);
          ctx.drawImage(img, 0, 0);
          this.frameToggle = !this.frameToggle;
          screen.style.opacity = this.frameToggle ? '0.9999' : '1';
          this.lastFrameVersion = res.version;
          this.lastRenderedQuality = targetQuality;
          if (targetQuality >= 90) {
            this.staticRefreshed = true;
          }
        } finally {
          URL.revokeObjectURL(objUrl);
        }
      }
    } catch {
      void 0;
    } finally {
      this.frameInFlight = false;
    }
  }

  private async refresh(): Promise<void> {
    await this.refreshStatus();
    await this.fetchAndRenderFrame();
  }

  private startFrameRafLoop(): void {
    const loop = (): void => {
      void this.fetchAndRenderFrame().finally(() => {
        try {
          requestAnimationFrame(loop);
        } catch {
          void 0;
        }
      });
    };
    try {
      requestAnimationFrame(loop);
    } catch {
      void 0;
    }
  }

  private handleHostDispatchedAction(action: string, payload: unknown): void {
    switch (action) {
      case 'ToggleConnectModal':
        this.setOverlay(this.store.overlay() === 'connect' ? 'none' : 'connect');
        break;
      case 'DisconnectRemote':
        this.disconnectRemote();
        break;
      case 'StartHostSharing':
        this.startHost();
        break;
      case 'StopHostSharing':
        this.stopHost();
        break;
      case 'ToggleToolsPopover':
        this.setOverlay(this.store.overlay() === 'tools' ? 'none' : 'tools');
        break;
      case 'SelectToolTab':
        if (
          payload === 'chat' ||
          payload === 'files' ||
          payload === 'clipboard' ||
          payload === 'audio'
        ) {
          this.setToolTab(payload);
        }
        break;
      case 'SetTheme':
        if (typeof payload === 'string') {
          this.setTheme(payload, true);
        }
        break;
      case 'CycleTheme':
        this.cycleTheme();
        break;
      case 'SetLanguage':
        if (typeof payload === 'string') {
          this.setLanguage(payload, true);
        }
        break;
      case 'SelectMonitor':
        if (payload === null || payload === undefined) {
          this.selectRemoteMonitors(null);
        } else if (typeof payload === 'number') {
          this.selectRemoteMonitors([payload]);
        }
        break;
      case 'SendSpecialKey':
        if (payload === 'super') {
          this.sendSuperKey();
        } else if (payload === 'ctrl_alt_del') {
          this.sendCtrlAltDel();
        } else if (payload === 'f9_next_monitor') {
          this.sendInput({ type: 'key', vk: 120, down: true });
          setTimeout(() => this.sendInput({ type: 'key', vk: 120, down: false }), 60);
        }
        break;
      case 'ToggleToolbarCollapse':
        this.setPillCollapsed(!this.store.pillCollapsed(), true);
        break;
      case 'SetToolbarCollapsed':
        this.setPillCollapsed(Boolean(payload), true);
        break;
      case 'ToggleFullscreen':
        this.toggleFullscreen();
        break;
      case 'CopyHostId':
        void this.copyHostId();
        break;
    }
  }
}
