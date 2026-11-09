import {
  AfterViewInit,
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  inject,
  signal,
  ViewChild,
} from '@angular/core';
import { UltraDeskSessionFacade } from '../../../application/facade/ultradesk-session.facade';
import { ToolTab } from '../../../domain/models/preferences.model';
import { FileTransferItem } from '../../../domain/models/session.model';

@Component({
  selector: 'ud-tools-popover',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div
      #popoverEl
      id="toolsPopover"
      class="tools-popover"
      [style.display]="store.overlay() === 'tools' ? 'flex' : 'none'"
    >
      <div #tabBarEl class="tab-bar">
        <div #tabIndEl class="m-tab-ind"></div>
        <button
          class="tab-btn"
          [class.active]="store.toolTab() === 'chat'"
          type="button"
          data-tool-tab="chat"
          (click)="onSelectTab('chat')"
        >
          {{ store.t('tab_chat') }}
        </button>
        <button
          class="tab-btn"
          [class.active]="store.toolTab() === 'files'"
          type="button"
          data-tool-tab="files"
          (click)="onSelectTab('files')"
        >
          {{ store.t('tab_files') }}
        </button>
        <button
          class="tab-btn"
          [class.active]="store.toolTab() === 'clipboard'"
          type="button"
          data-tool-tab="clipboard"
          (click)="onSelectTab('clipboard')"
        >
          {{ store.t('tab_clipboard') }}
        </button>
        <button
          class="tab-btn"
          [class.active]="store.toolTab() === 'audio'"
          type="button"
          data-tool-tab="audio"
          (click)="onSelectTab('audio')"
        >
          {{ store.t('tab_audio') }}
        </button>
      </div>

      <div class="tool-pane" [class.active]="store.toolTab() === 'chat'" data-tool-pane="chat">
        <div class="section-head">
          <span>{{ store.t('chat_header') }}</span>
        </div>
        <div id="chatBox" class="transfer-list">
          @if (store.chatMessages().length === 0) {
            <div class="muted">{{ store.t('chat_empty') }}</div>
          } @else {
            @for (msg of store.chatMessages(); track $index) {
              <div class="muted">{{ msg }}</div>
            }
          }
        </div>
        <div class="row">
          <input
            type="text"
            id="chatInput"
            [attr.placeholder]="store.t('chat_placeholder')"
            [value]="chatInput()"
            (input)="onChatInput($event)"
            (keydown.enter)="onSendChat($event)"
          />
          <button id="btnSendChat" type="button" (click)="onSendChat()">
            {{ store.t('send') }}
          </button>
        </div>
      </div>

      <div class="tool-pane" [class.active]="store.toolTab() === 'files'" data-tool-pane="files">
        <div class="section-head">
          <span>{{ store.t('files_header') }}</span>
        </div>
        <input #filePickerEl type="file" id="filePicker" />
        <button id="btnSendFile" type="button" (click)="onSendSelectedFile()">
          {{ store.t('send_selected_file') }}
        </button>
        <div class="row">
          <input
            type="text"
            id="filePathInput"
            class="mono"
            [attr.placeholder]="store.t('file_path_placeholder')"
            [value]="filePathInput()"
            (input)="onFilePathInput($event)"
          />
          <button id="btnSendPath" class="secondary" type="button" (click)="onSendPath()">
            {{ store.t('send') }}
          </button>
        </div>
        <div class="row">
          <input
            type="text"
            id="remoteFilePathInput"
            class="mono"
            [attr.placeholder]="store.t('remote_file_path_placeholder')"
            [value]="remoteFilePathInput()"
            (input)="onRemoteFilePathInput($event)"
            (keydown.enter)="onReceiveRemotePath($event)"
          />
          <button
            id="btnReceiveFile"
            class="secondary"
            type="button"
            (click)="onReceiveRemotePath()"
          >
            {{ store.t('receive_remote_file') }}
          </button>
        </div>
        <div id="transferList" class="transfer-list">
          @if (store.fileTransfers().length === 0) {
            <div class="muted">{{ store.t('no_transfers') }}</div>
          } @else {
            @for (tr of store.fileTransfers(); track tr.file_id) {
              <div class="transfer-item">
                <div style="display: flex; justify-content: space-between">
                  <strong>
                    {{
                      tr.direction === 'receive' ? store.t('dir_receive') : store.t('dir_send')
                    }}
                    · {{ tr.file_name }}
                  </strong>
                  <span class="muted">{{ tr.status }}</span>
                </div>
                <div class="progress">
                  <div [style.width.%]="transferPercent(tr)"></div>
                </div>
                @if (tr.direction === 'receive' && tr.saved_path) {
                  <div class="muted mono" style="font-size: 11px; word-break: break-all">
                    {{ tr.saved_path }}
                  </div>
                }
                <div style="display: flex; justify-content: space-between; align-items: center">
                  <span class="muted">
                    {{ tr.transferred_bytes }}/{{ tr.file_size }} B ({{ transferPercent(tr) }}%)
                  </span>
                  <span>
                    @if (tr.direction === 'receive' && tr.status === 'offered') {
                      <button type="button" (click)="facade.acceptTransfer(tr.file_id)">
                        {{ store.t('accept_file') }}
                      </button>
                    }
                    @if (tr.status === 'completed') {
                      <a class="dl-link" [href]="downloadHref(tr.file_id)">
                        {{ store.t('download_file') }}
                      </a>
                    }
                    @if (tr.status === 'offered' || tr.status === 'transferring') {
                      <button
                        type="button"
                        class="danger"
                        (click)="facade.cancelTransfer(tr.file_id)"
                      >
                        {{ store.t('cancel_file') }}
                      </button>
                    }
                  </span>
                </div>
              </div>
            }
          }
        </div>
      </div>

      <div
        class="tool-pane"
        [class.active]="store.toolTab() === 'clipboard'"
        data-tool-pane="clipboard"
      >
        <div class="section-head">
          <span>{{ store.t('unicode_header') }}</span>
        </div>
        <div class="row">
          <input
            type="text"
            id="unicodeTextInput"
            [attr.placeholder]="store.t('unicode_placeholder')"
            [value]="unicodeInput()"
            (input)="onUnicodeInput($event)"
            (keydown.enter)="onSendUnicode($event)"
          />
          <button id="btnSendUnicode" class="secondary" type="button" (click)="onSendUnicode()">
            {{ store.t('type_btn') }}
          </button>
        </div>
        <div class="divider"></div>
        <div class="section-head">
          <span>{{ store.t('clip_header') }}</span>
          <span id="clipStats" class="muted mono">{{ store.clipStatsText() }}</span>
        </div>
        <textarea
          id="clipInput"
          rows="2"
          [attr.placeholder]="store.t('clip_input_placeholder')"
          [value]="store.clipInputText()"
          (input)="onClipInput($event)"
        ></textarea>
        <div class="row">
          <button
            id="btnSendClip"
            type="button"
            style="flex: 1"
            (click)="facade.sendClipboardToPartner(store.clipInputText())"
          >
            {{ store.t('send_to_partner') }}
          </button>
          <button
            id="btnSyncSysClip"
            class="secondary"
            type="button"
            (click)="facade.pullLinuxClipboard()"
          >
            {{ store.t('pull_linux_clip') }}
          </button>
        </div>
        <span class="field-label">{{ store.t('remote_clip_label') }}</span>
        <textarea
          id="clipRemote"
          rows="2"
          readonly
          [attr.placeholder]="store.t('clip_remote_placeholder')"
          [value]="store.remoteClipboardText()"
        ></textarea>
      </div>

      <div class="tool-pane" [class.active]="store.toolTab() === 'audio'" data-tool-pane="audio">
        <div class="section-head">
          <span>{{ store.t('audio_header') }}</span>
          <span id="audioStats" class="muted mono">{{ store.audioStatsText() }}</span>
        </div>
        <div class="row">
          <button id="btnAudioToggle" type="button" style="flex: 1" (click)="facade.enableAudio()">
            {{ store.t('audio_enable') }}
          </button>
          <button id="btnAudioMute" class="secondary" type="button" (click)="facade.muteAudio()">
            {{ store.t('audio_mute') }}
          </button>
        </div>
        <div id="audioStatus" class="muted">{{ store.audioStatusText() }}</div>
      </div>
    </div>
  `,
})
export class ToolsPopoverComponent implements AfterViewInit {
  @ViewChild('popoverEl', { static: true })
  private readonly popoverRef!: ElementRef<HTMLDivElement>;

  @ViewChild('tabBarEl', { static: true })
  private readonly tabBarRef!: ElementRef<HTMLDivElement>;

  @ViewChild('tabIndEl', { static: true })
  private readonly tabIndRef!: ElementRef<HTMLDivElement>;

  @ViewChild('filePickerEl', { static: true })
  private readonly filePickerRef!: ElementRef<HTMLInputElement>;

  protected readonly facade = inject(UltraDeskSessionFacade);
  protected readonly store = this.facade.store;

  protected readonly chatInput = signal<string>('');
  protected readonly filePathInput = signal<string>('');
  protected readonly remoteFilePathInput = signal<string>('');
  protected readonly unicodeInput = signal<string>('');

  ngAfterViewInit(): void {
    this.facade.registerPopoverLayoutHook(() => {
      this.positionToolsPopover();
      this.syncTabIndicator(true);
    });
  }

  positionToolsPopover(): void {
    if (this.store.overlay() !== 'tools') {
      return;
    }
    const tp = this.popoverRef.nativeElement;
    const btnTools = document.querySelector<HTMLElement>('#btnToggleTools');
    if (btnTools) {
      const tr = btnTools.getBoundingClientRect();
      const popW = tp.offsetWidth || 360;
      tp.style.top = `${Math.max(4, tr.bottom + 4)}px`;
      const left = Math.max(8, Math.min(window.innerWidth - popW - 8, tr.right - popW));
      tp.style.left = `${left}px`;
      tp.style.right = 'auto';
    } else {
      tp.style.top = '44px';
      tp.style.right = '16px';
      tp.style.left = 'auto';
    }
  }

  syncTabIndicator(immediate = false): void {
    const tabBar = this.tabBarRef.nativeElement;
    const ind = this.tabIndRef.nativeElement;
    const activeBtn = tabBar.querySelector<HTMLElement>('.tab-btn.active');
    if (!activeBtn) {
      return;
    }
    const bRect = activeBtn.getBoundingClientRect();
    const barRect = tabBar.getBoundingClientRect();
    if (barRect.width === 0) {
      return;
    }
    if (immediate) {
      ind.style.transition = 'none';
    }
    const x = bRect.left - barRect.left;
    const y = bRect.top - barRect.top;
    ind.style.transform = `translate(${Math.round(x)}px, ${Math.round(y)}px)`;
    ind.style.width = `${Math.round(bRect.width)}px`;
    ind.style.height = `${Math.round(bRect.height)}px`;
    if (immediate) {
      void ind.offsetWidth;
      ind.style.transition = '';
    }
  }

  protected onSelectTab(tab: ToolTab): void {
    this.facade.setToolTab(tab);
    setTimeout(() => {
      this.positionToolsPopover();
      this.syncTabIndicator(false);
    }, 0);
  }

  protected onChatInput(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.chatInput.set(target?.value ?? '');
  }

  protected async onSendChat(event?: Event): Promise<void> {
    event?.preventDefault();
    const text = this.chatInput().trim();
    if (!text) {
      return;
    }
    this.chatInput.set('');
    await this.facade.sendChatMessage(text);
  }

  protected async onSendSelectedFile(): Promise<void> {
    const file = this.filePickerRef.nativeElement.files?.[0];
    await this.facade.sendSelectedFile(file);
  }

  protected onFilePathInput(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.filePathInput.set(target?.value ?? '');
  }

  protected async onSendPath(): Promise<void> {
    await this.facade.sendFileByPath(this.filePathInput());
  }

  protected onRemoteFilePathInput(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.remoteFilePathInput.set(target?.value ?? '');
  }

  protected async onReceiveRemotePath(event?: Event): Promise<void> {
    event?.preventDefault();
    const path = this.remoteFilePathInput().trim();
    if (!path) {
      return;
    }
    this.remoteFilePathInput.set('');
    await this.facade.receiveRemoteFileByPath(path);
  }

  protected onUnicodeInput(event: Event): void {
    const target = event.target as HTMLInputElement | null;
    this.unicodeInput.set(target?.value ?? '');
  }

  protected async onSendUnicode(event?: Event): Promise<void> {
    event?.preventDefault();
    const text = this.unicodeInput();
    if (!text) {
      return;
    }
    this.unicodeInput.set('');
    await this.facade.sendUnicodeText(text);
  }

  protected onClipInput(event: Event): void {
    const target = event.target as HTMLTextAreaElement | null;
    this.store.clipInputText.set(target?.value ?? '');
  }

  protected transferPercent(tr: FileTransferItem): number {
    if (tr.file_size <= 0) {
      return 100;
    }
    return Math.min(100, Math.round((tr.transferred_bytes * 100) / tr.file_size));
  }

  protected downloadHref(fileId: string): string {
    return `/files/download?file_id=${encodeURIComponent(fileId)}`;
  }
}
