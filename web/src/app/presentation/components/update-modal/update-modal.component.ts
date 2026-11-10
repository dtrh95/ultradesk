import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { UltraDeskSessionFacade } from '../../../application/facade/ultradesk-session.facade';

@Component({
  selector: 'ud-update-modal',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div
      id="updateModal"
      class="modal-backdrop"
      [style.display]="store.overlay() === 'update' ? 'flex' : 'none'"
      tabindex="-1"
      (click)="onBackdropClick($event)"
      (keydown.escape)="onEscape()"
    >
      <div class="spotlight-card update-modal-card">
        <div class="update-modal-header">
          <h2 class="update-modal-title">
            <span>{{ store.t('update_title') }}</span>
            <span class="update-target-chip">{{ updateStatus()?.install_target || 'Linux' }}</span>
          </h2>
          <div class="update-version-row">
            <span class="current-ver">v{{ updateStatus()?.current_version || '1.0.7' }}</span>
            <span class="arrow">→</span>
            <span class="latest-ver">v{{ updateStatus()?.latest_version || '---' }}</span>
          </div>
        </div>

        <div id="updateNotesBox" class="update-notes-box">
          {{ updateStatus()?.release_notes || store.t('update_notes') }}
        </div>

        <div class="update-asset-info">
          @if (updateStatus()?.asset_name; as asset) {
            <div>{{ asset }}</div>
          }
          @if (updateStatus()?.staged_path; as staged) {
            <div>{{ staged }}</div>
          }
        </div>

        @if (isDownloadingOrApplying()) {
          <div
            id="updateProgressBar"
            class="update-progress-wrap"
            role="progressbar"
            [attr.aria-valuenow]="progressPct()"
            aria-valuemin="0"
            aria-valuemax="100"
          >
            <div class="update-progress-fill" [style.width.%]="progressPct()"></div>
          </div>
        }

        @if (statusMessage(); as msg) {
          <div class="update-status-msg">{{ msg }}</div>
        }

        @if (updateStatus()?.error; as err) {
          <div class="update-error-msg">{{ err }}</div>
        }

        <div class="update-actions-row">
          <button
            id="btnLaterUpdate"
            class="secondary"
            type="button"
            [disabled]="isBusy()"
            (click)="onLater()"
          >
            {{ store.t('update_later') }}
          </button>
          <button
            id="btnConfirmUpdate"
            class="action-btn"
            type="button"
            [disabled]="isBusy()"
            (click)="onConfirm()"
          >
            {{ confirmButtonLabel() }}
          </button>
        </div>
      </div>
    </div>
  `,
})
export class UpdateModalComponent {
  protected readonly facade = inject(UltraDeskSessionFacade);
  protected readonly store = this.facade.store;

  protected readonly updateStatus = computed(() => this.store.updateStatus());

  protected readonly isDownloadingOrApplying = computed(() => {
    const s = this.updateStatus()?.state;
    return s === 'downloading' || s === 'applying';
  });

  protected readonly isBusy = computed(() => {
    const s = this.updateStatus()?.state;
    return s === 'downloading' || s === 'applying';
  });

  protected readonly progressPct = computed(() => {
    const s = this.updateStatus();
    if (s?.state === 'downloading') {
      return s.download_progress_pct ?? 0;
    }
    if (s?.state === 'applying' || s?.state === 'ready' || s?.state === 'applied') {
      return 100;
    }
    return 0;
  });

  protected readonly statusMessage = computed(() => {
    const s = this.updateStatus();
    if (!s) {
      return '';
    }
    if (s.message) {
      return s.message;
    }
    switch (s.state) {
      case 'downloading':
        return this.store.t('update_downloading', { pct: s.download_progress_pct ?? 0 });
      case 'applying':
        return this.store.t('update_applying');
      case 'applied':
        return this.store.t('update_applied');
      case 'up_to_date':
        return this.store.t('update_up_to_date');
      default:
        return '';
    }
  });

  protected readonly confirmButtonLabel = computed(() => {
    const s = this.updateStatus();
    if (s?.state === 'downloading') {
      return this.store.t('update_downloading', { pct: s.download_progress_pct ?? 0 });
    }
    if (s?.state === 'applying') {
      return this.store.t('update_applying');
    }
    if (s?.state === 'applied') {
      return this.store.t('update_close');
    }
    return this.store.t('update_confirm');
  });

  onBackdropClick(event: MouseEvent): void {
    if (event.target === event.currentTarget && !this.isBusy()) {
      void this.facade.dismissUpdate();
    }
  }

  onEscape(): void {
    if (!this.isBusy()) {
      void this.facade.dismissUpdate();
    }
  }

  onLater(): void {
    if (!this.isBusy()) {
      void this.facade.dismissUpdate();
    }
  }

  onConfirm(): void {
    if (this.updateStatus()?.state === 'applied') {
      void this.facade.dismissUpdate();
      return;
    }
    void this.facade.confirmUpdate();
  }
}
