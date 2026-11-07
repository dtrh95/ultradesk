import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { UltraDeskSessionFacade } from '../../../application/facade/ultradesk-session.facade';

@Component({
  selector: 'ud-top-pill-toolbar',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <header
      id="topPill"
      class="top-pill menu-toolbar"
      [class.collapsed]="store.pillCollapsed()"
      role="menubar"
    >
      <button
        id="pillDragHandle"
        class="pill-drag-handle"
        type="button"
        [attr.title]="store.t('drag_pill_title')"
        (dblclick)="onDragHandleDblClick($event)"
      >
        <svg class="svg-icon" viewBox="0 0 24 24">
          <rect x="4" y="4" width="16" height="16" rx="2" />
          <path d="M4 9h16" />
        </svg>
      </button>
      <div id="pillExpandedContent" class="pill-expanded-content">
        <div class="toolbar-cluster left-cluster">
          <span class="pill-brand">UltraDesk</span>
          <button
            id="btnToggleConnect"
            class="pill-btn"
            [class.active]="store.overlay() === 'connect'"
            type="button"
            [attr.data-conn-state]="store.connectionState()"
            (click)="onToggleConnect()"
          >
            <svg class="svg-icon" viewBox="0 0 24 24">
              <path d="M9 12l2 2l4 -4" />
              <path
                d="M12 3a12 12 0 0 0 8.5 3a12 12 0 0 1 -8.5 15a12 12 0 0 1 -8.5 -15a12 12 0 0 0 8.5 -3"
              />
            </svg>
            <span id="status">{{ store.statusText() }}</span>
          </button>
          <span
            id="e2eBadge"
            class="status-badge e2e-badge"
            [style.display]="store.activeE2e() ? 'inline-flex' : 'none'"
            [attr.title]="store.e2eTooltip()"
          >
            {{ store.e2eLabel() }}
          </span>
        </div>
        <div class="pill-sep"></div>
        <div class="toolbar-cluster center-cluster">
          <div
            id="monitorQuickBar"
            class="monitor-quick-bar"
            [style.display]="store.remoteMonitorsView().items.length > 1 ? 'inline-flex' : 'none'"
          >
            @if (store.remoteMonitorsView().items.length > 1) {
              <button
                type="button"
                class="mon-chip"
                [class.active]="store.remoteMonitorsView().allActive"
                [attr.title]="store.t('all_monitors')"
                (click)="onRemoteMonitorChipClick(-1, $event)"
              >
                {{ store.t('all_monitors_short') }}
              </button>
              @for (item of store.remoteMonitorsView().items; track item.index) {
                <button
                  type="button"
                  class="mon-chip"
                  [class.active]="item.active"
                  [attr.title]="
                    store.t('monitor_item', {
                      idx: item.index + 1,
                      w: item.width,
                      h: item.height,
                    }) + ' (Shift/Ctrl)'
                  "
                  (click)="onRemoteMonitorChipClick(item.index, $event)"
                >
                  {{ store.t('monitor_short', { idx: item.index + 1 }) }}
                </button>
              }
            }
          </div>
          <select
            id="monitorSelect"
            class="pill-select"
            [attr.title]="store.t('monitor_title')"
            [value]="store.remoteMonitorSelectValue()"
            (change)="onMonitorSelectChange($event)"
          >
            <option value="" [selected]="store.remoteMonitorsView().allActive">
              {{ store.t('all_monitors') }} ({{ store.remoteMonitorsView().items.length }})
            </option>
            @for (item of store.remoteMonitorsView().items; track item.index) {
              <option
                [value]="item.index"
                [selected]="
                  !store.remoteMonitorsView().allActive &&
                  store.remoteMonitorsView().selected.length === 1 &&
                  store.remoteMonitorsView().selected[0] === item.index
                "
              >
                {{
                  store.t('monitor_item', {
                    idx: item.index + 1,
                    w: item.width,
                    h: item.height,
                  })
                }}
              </option>
            }
            @if (store.remoteMonitorSubsetOption(); as subsetOpt) {
              <option [value]="subsetOpt.value" selected>{{ subsetOpt.label }}</option>
            }
          </select>
          <button
            id="btnWinKey"
            class="pill-btn"
            type="button"
            [attr.title]="store.t('win_key_title')"
            (click)="facade.sendSuperKey()"
          >
            <svg class="svg-icon" viewBox="0 0 24 24">
              <rect x="4" y="4" width="6" height="6" rx="1" />
              <rect x="14" y="4" width="6" height="6" rx="1" />
              <rect x="4" y="14" width="6" height="6" rx="1" />
              <rect x="14" y="14" width="6" height="6" rx="1" />
            </svg>
            <span>Super</span>
          </button>
          <button
            id="btnToggleTools"
            class="pill-btn"
            [class.active]="store.overlay() === 'tools'"
            type="button"
            (click)="onToggleTools()"
          >
            <svg class="svg-icon" viewBox="0 0 24 24">
              <path d="M4 6h16" />
              <path d="M4 12h16" />
              <path d="M4 18h12" />
            </svg>
            <span>{{ store.t('nav_tools') }}</span>
          </button>
        </div>
        <div class="pill-sep"></div>
        <div class="toolbar-cluster right-cluster">
          <button
            id="btnCycleTheme"
            class="pill-btn"
            type="button"
            [attr.title]="store.t('btn_theme_title')"
            (click)="onCycleTheme($event)"
          >
            <svg id="themeBtnIcon" class="svg-icon" viewBox="0 0 24 24">
              @if (store.themeMode() === 'auto-time') {
                <circle cx="12" cy="12" r="9" />
                <path d="M12 7v5l3 3" />
              } @else if (store.isLightTheme()) {
                <circle cx="12" cy="12" r="4" />
                <path
                  d="M3 12h1m8 -9v1m8 8h1m-9 8v1m-6.4 -15.4l.7 .7m12.1 -.7l-.7 .7m0 11.4l.7 .7m-12.1 -.7l-.7 .7"
                />
              } @else {
                <path
                  d="M12 3c.132 0 .263 0 .393 0a7.5 7.5 0 0 0 7.92 12.446a9 9 0 1 1 -8.313 -12.454z"
                />
              }
            </svg>
            <span id="themeBtnLabel">{{ store.themeShortLabel() }}</span>
          </button>
          <select
            id="themeSelect"
            class="pill-select"
            [attr.title]="store.t('theme_title')"
            [value]="store.themeMode()"
            (change)="onThemeSelectChange($event)"
          >
            <option value="auto-time" [selected]="store.themeMode() === 'auto-time'">
              {{ store.t('theme_auto_time') }}
            </option>
            <option value="auto-os" [selected]="store.themeMode() === 'auto-os'">
              {{ store.t('theme_auto_os') }}
            </option>
            <option value="onedark" [selected]="store.themeMode() === 'onedark'">
              {{ store.t('theme_onedark') }}
            </option>
            <option value="onelight" [selected]="store.themeMode() === 'onelight'">
              {{ store.t('theme_onelight') }}
            </option>
            <option value="vscode-dark" [selected]="store.themeMode() === 'vscode-dark'">
              {{ store.t('theme_vscode_dark') }}
            </option>
            <option value="vscode-light" [selected]="store.themeMode() === 'vscode-light'">
              {{ store.t('theme_vscode_light') }}
            </option>
            <option value="gruvbox-dark" [selected]="store.themeMode() === 'gruvbox-dark'">
              {{ store.t('theme_gruvbox_dark') }}
            </option>
            <option value="gruvbox-light" [selected]="store.themeMode() === 'gruvbox-light'">
              {{ store.t('theme_gruvbox_light') }}
            </option>
          </select>
          <div class="lang-switch">
            <button
              id="btnLangVi"
              class="lang-btn"
              [class.active]="store.lang() === 'vi'"
              type="button"
              (click)="facade.setLanguage('vi', true)"
            >
              VI
            </button>
            <button
              id="btnLangEn"
              class="lang-btn"
              [class.active]="store.lang() === 'en'"
              type="button"
              (click)="facade.setLanguage('en', true)"
            >
              EN
            </button>
          </div>
          <button
            id="btnFullscreen"
            class="pill-btn"
            type="button"
            [attr.title]="store.t('fullscreen_title')"
            (click)="facade.toggleFullscreen()"
          >
            <svg class="svg-icon" viewBox="0 0 24 24">
              <path d="M4 8v-4h4" />
              <path d="M16 4h4v4" />
              <path d="M4 16v4h4" />
              <path d="M16 20h4v-4" />
            </svg>
          </button>
          <button
            id="btnCollapsePill"
            class="pill-btn"
            type="button"
            style="padding: 5px 7px"
            [attr.title]="store.t('hide_pill_title')"
            (click)="facade.setPillCollapsed(true, true)"
          >
            <svg class="svg-icon" viewBox="0 0 24 24"><path d="M6 15l6 -6l6 6" /></svg>
          </button>
        </div>
      </div>
      <button
        id="btnRestorePill"
        class="pill-restore-tab"
        type="button"
        [attr.title]="store.t('show_pill_title')"
        (click)="facade.setPillCollapsed(false, true)"
      >
        <span
          id="restoreStatusDot"
          class="restore-dot"
          [style.background]="restoreDotColor()"
          [style.color]="restoreDotColor()"
        ></span>
        <svg class="svg-icon" viewBox="0 0 24 24"><path d="M6 9l6 6l6 -6" /></svg>
      </button>
    </header>
  `,
})
export class TopPillToolbarComponent {
  protected readonly facade = inject(UltraDeskSessionFacade);
  protected readonly store = this.facade.store;

  protected readonly restoreDotColor = computed(() => {
    const state = this.store.connectionState();
    if (state === 'connected') {
      return 'var(--status-ok)';
    }
    if (state === 'connecting') {
      return 'var(--secondary)';
    }
    return 'var(--text-dim)';
  });

  protected onToggleConnect(): void {
    this.facade.setOverlay(this.store.overlay() === 'connect' ? 'none' : 'connect');
  }

  protected onToggleTools(): void {
    this.facade.setOverlay(this.store.overlay() === 'tools' ? 'none' : 'tools');
  }

  protected onDragHandleDblClick(event: MouseEvent): void {
    event.preventDefault();
    event.stopPropagation();
    this.facade.setPillCollapsed(!this.store.pillCollapsed(), true);
  }

  protected onRemoteMonitorChipClick(idx: number, event: MouseEvent): void {
    this.facade.handleRemoteMonitorClick(idx, event);
  }

  protected onMonitorSelectChange(event: Event): void {
    const target = event.target as HTMLSelectElement | null;
    if (target) {
      this.facade.handleMonitorDropdownChange(target.value);
    }
  }

  protected onCycleTheme(event: MouseEvent): void {
    this.facade.cycleTheme(Boolean(event.shiftKey));
  }

  protected onThemeSelectChange(event: Event): void {
    const target = event.target as HTMLSelectElement | null;
    if (target) {
      this.facade.setTheme(target.value, true);
    }
  }
}
