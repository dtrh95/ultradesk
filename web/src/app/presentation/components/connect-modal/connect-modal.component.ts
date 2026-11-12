import { ChangeDetectionStrategy, Component, inject, signal } from '@angular/core';
import { UltraDeskSessionFacade } from '../../../application/facade/ultradesk-session.facade';
import { SavedRemote } from '../../../domain/models/session.model';
import { formatPartnerIdForDisplay } from '../../../domain/services/partner-target.domain-service';

@Component({
  selector: 'ud-connect-modal',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div
      id="connectModal"
      class="modal-backdrop"
      [style.display]="store.overlay() === 'connect' ? 'flex' : 'none'"
      tabindex="-1"
      (click)="onBackdropClick($event)"
      (keydown.escape)="facade.setOverlay('none')"
    >
      <div class="spotlight-card">
        <div class="pane host">
          <div class="pane-head">
            <div class="pane-title">
              <svg class="svg-icon" viewBox="0 0 24 24">
                <rect x="3" y="4" width="18" height="12" rx="1" />
                <path d="M7 20h10" />
                <path d="M9 16v4" />
                <path d="M15 16v4" />
              </svg>
              <span>{{ store.t('host_title') }}</span>
            </div>
            <span id="hostPhaseBadge" class="badge">{{ store.hostPhaseBadge() }}</span>
          </div>
          <div class="cred-box">
            <div class="cred-meta">
              <span class="cred-label">{{ store.t('your_id') }}</span>
              <span id="hostIdVal" class="cred-val">{{ store.hostIdVal() }}</span>
            </div>
            <button
              id="btnCopyHostId"
              class="secondary"
              [class.is-copied]="store.copyConfirmed()"
              type="button"
              (click)="facade.copyHostId()"
            >
              @if (store.copyConfirmed()) {
                <svg class="svg-icon" viewBox="0 0 24 24"><path d="M5 12l5 5l10 -10" /></svg>
                <span>{{ store.t('copied') }}</span>
              } @else {
                {{ store.t('copy') }}
              }
            </button>
          </div>
          <div class="cred-box">
            <div class="cred-meta">
              <span class="cred-label">{{ store.t('temp_pass') }}</span>
              <span id="hostPassVal" class="cred-val pass">{{ store.hostPassVal() }}</span>
            </div>
            <div class="row">
              <button id="btnHostStart" type="button" (click)="facade.startHost()">
                {{ store.t('host_start') }}
              </button>
              <button id="btnHostStop" class="secondary" type="button" (click)="facade.stopHost()">
                {{ store.t('host_stop') }}
              </button>
            </div>
          </div>
          <div
            id="hostApprovalBox"
            class="approval-box"
            [style.display]="store.hostApprovalVisible() ? 'flex' : 'none'"
          >
            <div id="hostViewerPrompt" class="approval-prompt">
              {{ store.hostViewerPrompt() }}
            </div>
            <div class="row">
              <button
                id="btnHostApproveCtrl"
                type="button"
                style="flex: 1"
                (click)="facade.approveHost(true)"
              >
                {{ store.t('approve_ctrl') }}
              </button>
              <button
                id="btnHostApproveView"
                class="secondary"
                type="button"
                (click)="facade.approveHost(false)"
              >
                {{ store.t('approve_view') }}
              </button>
              <button
                id="btnHostDecline"
                class="danger"
                type="button"
                (click)="facade.declineHost()"
              >
                {{ store.t('decline') }}
              </button>
            </div>
          </div>
          <div class="row">
            <input
              type="password"
              id="masterPassInput"
              class="mono"
              [attr.placeholder]="store.t('master_pass_placeholder')"
              [value]="masterPassInput()"
              (input)="onMasterPassInput($event)"
            />
            <button
              id="btnSetMasterPass"
              class="secondary"
              type="button"
              (click)="onSaveMasterPass()"
            >
              {{ store.t('save') }}
            </button>
          </div>
          <div class="col">
            <span class="field-label">{{ store.t('host_monitors_label') }}</span>
            <div id="hostMonitorsBox" class="mon-grid">
              @if (store.hostMonitorsView().items.length === 0) {
                <span class="muted">{{ store.t('remote_monitors_empty') }}</span>
              } @else {
                <button
                  type="button"
                  class="mon-card"
                  [class.active]="store.hostMonitorsView().allActive"
                  (click)="facade.handleHostMonitorClick(-1, $event)"
                >
                  <span>
                    {{ store.t('all_monitors_short') }} ({{
                      store.hostMonitorsView().items.length
                    }})
                  </span>
                </button>
                @for (item of store.hostMonitorsView().items; track item.index) {
                  <button
                    type="button"
                    class="mon-card"
                    [class.active]="item.active"
                    (click)="facade.handleHostMonitorClick(item.index, $event)"
                  >
                    <input
                      type="checkbox"
                      [checked]="item.checked"
                      [attr.title]="store.t('monitor_title')"
                      (click)="facade.handleHostMonitorCheck(item.index, $event)"
                    />
                    <span>
                      {{
                        store.t('monitor_item', {
                          idx: item.index + 1,
                          w: item.width,
                          h: item.height,
                        })
                      }}
                    </span>
                  </button>
                }
              }
            </div>
          </div>
          <div id="hostStatusInfo" class="muted">{{ store.hostStatusInfoText() }}</div>
          @if (store.showMacosPermissionBox()) {
            <div id="macosPermissionsBox" class="approval-box">
              <div class="field-label">{{ store.t('macos_perm_title') }}</div>
              <div class="row" style="justify-content: space-between; align-items: center">
                <span>{{ store.t('macos_perm_screen') }}</span>
                <div class="row">
                  <span class="badge">
                    {{
                      store.macosPermissions()?.screen_recording === 'granted'
                        ? store.t('macos_perm_granted')
                        : store.t('macos_perm_missing')
                    }}
                  </span>
                  @if (store.macosPermissions()?.screen_recording === 'not_granted') {
                    <button
                      id="btnGrantScreenPerm"
                      type="button"
                      (click)="facade.requestMacOsPermission('screen_recording')"
                    >
                      {{ store.t('macos_perm_grant_btn') }}
                    </button>
                    <button
                      id="btnOpenScreenSettings"
                      class="secondary"
                      type="button"
                      (click)="facade.openMacOsPermissionSettings('screen_recording')"
                    >
                      {{ store.t('macos_perm_settings_btn') }}
                    </button>
                  }
                </div>
              </div>
              <div class="row" style="justify-content: space-between; align-items: center">
                <span>{{ store.t('macos_perm_ax') }}</span>
                <div class="row">
                  <span class="badge">
                    {{
                      store.macosPermissions()?.accessibility === 'granted'
                        ? store.t('macos_perm_granted')
                        : store.t('macos_perm_missing')
                    }}
                  </span>
                  @if (store.macosPermissions()?.accessibility === 'not_granted') {
                    <button
                      id="btnGrantAxPerm"
                      type="button"
                      (click)="facade.requestMacOsPermission('accessibility')"
                    >
                      {{ store.t('macos_perm_grant_btn') }}
                    </button>
                    <button
                      id="btnOpenAxSettings"
                      class="secondary"
                      type="button"
                      (click)="facade.openMacOsPermissionSettings('accessibility')"
                    >
                      {{ store.t('macos_perm_settings_btn') }}
                    </button>
                  }
                </div>
              </div>
            </div>
          }
        </div>

        <div class="pane remote">
          <div class="pane-head">
            <div class="pane-title">
              <svg class="svg-icon" viewBox="0 0 24 24">
                <path d="M15 3h6v6" />
                <path d="M10 14l11 -11" />
                <path d="M18 13v6a2 2 0 0 1 -2 2h-11a2 2 0 0 1 -2 -2v-11a2 2 0 0 1 2 -2h6" />
              </svg>
              <span>{{ store.t('remote_title') }}</span>
            </div>
            <label class="check-label">
              <input
                id="rememberCreds"
                type="checkbox"
                [checked]="store.rememberCreds()"
                (change)="onRememberCredsChange($event)"
              />
              <span>{{ store.t('remember_pass') }}</span>
            </label>
          </div>
          <div class="row">
            <div class="col">
              <label class="field-label" for="partnerId">{{ store.t('partner_id_label') }}</label>
              <input
                id="partnerId"
                class="mono"
                type="text"
                [attr.placeholder]="store.t('partner_id_placeholder')"
                [value]="store.partnerId()"
                (input)="onPartnerIdInput($event)"
                (paste)="onPartnerIdPaste($event)"
                (blur)="onPartnerIdBlur()"
              />
            </div>
            <div class="col">
              <label class="field-label" for="partnerPass">{{
                store.t('partner_pass_label')
              }}</label>
              <input
                id="partnerPass"
                class="mono"
                type="password"
                [attr.placeholder]="store.t('partner_pass_placeholder')"
                [value]="store.partnerPass()"
                (input)="onPartnerPassInput($event)"
              />
            </div>
          </div>
          <div class="col">
            <label class="field-label" for="partnerAlias">{{
              store.t('partner_alias_label')
            }}</label>
            <div class="row">
              <input
                id="partnerAlias"
                type="text"
                maxlength="64"
                [attr.placeholder]="store.t('partner_alias_placeholder')"
                [value]="store.partnerAlias()"
                (input)="onPartnerAliasInput($event)"
                (keydown.enter)="onSaveAlias()"
              />
              <button
                id="btnSaveAlias"
                class="secondary"
                type="button"
                [disabled]="!store.normalizedPartnerId()"
                [attr.title]="store.t('save_alias_title')"
                (click)="onSaveAlias()"
              >
                {{ store.t('save') }}
              </button>
            </div>
          </div>
          <div class="row">
            <button id="connect" type="button" style="flex: 1" (click)="facade.connectRemote()">
              {{ store.t('btn_connect') }}
            </button>
            <button
              id="disconnect"
              class="danger"
              type="button"
              (click)="facade.disconnectRemote()"
            >
              {{ store.t('btn_disconnect') }}
            </button>
          </div>
          <div class="col">
            <label class="field-label" for="savedRemotes">{{
              store.t('saved_remotes_label')
            }}</label>
            <select
              id="savedRemotes"
              [attr.title]="store.t('saved_remotes_title')"
              (change)="onSavedRemoteSelect($event)"
            >
              <option value="">
                {{ store.t('saved_remotes_opt', { count: store.savedRemotes().length }) }}
              </option>
              @for (remote of store.savedRemotes(); track remote.partner_id) {
                <option
                  [value]="remote.partner_id"
                  [selected]="remote.partner_id === store.normalizedPartnerId()"
                >
                  {{ remoteOptionLabel(remote) }}
                </option>
              }
            </select>
          </div>
          <div class="col">
            <span class="field-label">{{ store.t('remote_monitors_label') }}</span>
            <div id="remoteMonitorsBox" class="mon-grid">
              @if (store.remoteMonitorsView().items.length === 0) {
                <span class="muted">{{ store.t('remote_monitors_empty') }}</span>
              } @else {
                <button
                  type="button"
                  class="mon-card"
                  [class.active]="store.remoteMonitorsView().allActive"
                  (click)="facade.handleRemoteMonitorClick(-1, $event)"
                >
                  <span>
                    {{ store.t('all_monitors_short') }} ({{
                      store.remoteMonitorsView().items.length
                    }})
                  </span>
                </button>
                @for (item of store.remoteMonitorsView().items; track item.index) {
                  <button
                    type="button"
                    class="mon-card"
                    [class.active]="item.active"
                    (click)="facade.handleRemoteMonitorClick(item.index, $event)"
                  >
                    <input
                      type="checkbox"
                      [checked]="item.checked"
                      [attr.title]="store.t('monitor_title')"
                      (click)="facade.handleRemoteMonitorCheck(item.index, $event)"
                    />
                    <span>
                      {{
                        store.t('monitor_item', {
                          idx: item.index + 1,
                          w: item.width,
                          h: item.height,
                        })
                      }}
                    </span>
                  </button>
                }
              }
            </div>
          </div>
        </div>
      </div>
    </div>
  `,
})
export class ConnectModalComponent {
  protected readonly facade = inject(UltraDeskSessionFacade);
  protected readonly store = this.facade.store;
  protected readonly masterPassInput = signal<string>('');

  protected onBackdropClick(event: MouseEvent): void {
    const target = event.target as HTMLElement | null;
    if (target?.id === 'connectModal') {
      this.facade.setOverlay('none');
    }
  }

  protected onMasterPassInput(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.masterPassInput.set(target?.value ?? '');
  }

  protected async onSaveMasterPass(): Promise<void> {
    const pw = this.masterPassInput();
    await this.facade.saveMasterPassword(pw);
    this.masterPassInput.set('');
  }

  protected onPartnerIdInput(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.facade.applyPartnerIdInput(target?.value ?? '', false);
  }

  protected onPartnerIdPaste(event: ClipboardEvent): void {
    event.preventDefault();
    const pasted = event.clipboardData?.getData('text') ?? '';
    this.facade.applyPartnerIdInput(pasted, true);
  }

  protected onPartnerIdBlur(): void {
    this.facade.applyPartnerIdInput(this.store.partnerId(), true);
  }

  protected onPartnerAliasInput(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.store.partnerAlias.set(target?.value ?? '');
  }

  protected onSaveAlias(): void {
    void this.facade.savePartnerAlias();
  }

  protected onPartnerPassInput(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.store.partnerPass.set(target?.value ?? '');
  }

  protected onRememberCredsChange(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.facade.updateRememberCreds(Boolean(target?.checked));
  }

  protected onSavedRemoteSelect(event: Event): void {
    const target = event.target as HTMLSelectElement | null;
    if (target) {
      this.facade.selectSavedRemote(target.value);
    }
  }

  protected remoteOptionLabel(remote: SavedRemote): string {
    const alias = remote.alias?.trim();
    if (alias && alias.length > 0) {
      return alias;
    }
    return formatPartnerIdForDisplay(remote.partner_id);
  }
}
