import {
  ChangeDetectionStrategy,
  Component,
  inject,
  OnDestroy,
  OnInit,
  ViewChild,
} from '@angular/core';
import { UltraDeskSessionFacade } from './application/facade/ultradesk-session.facade';
import { resolveVirtualKeyCode } from './domain/services/keymap.domain-service';
import { CanvasStageComponent } from './presentation/components/canvas-stage/canvas-stage.component';
import { ConnectModalComponent } from './presentation/components/connect-modal/connect-modal.component';
import { ToolsPopoverComponent } from './presentation/components/tools-popover/tools-popover.component';
import { TopPillToolbarComponent } from './presentation/components/top-pill-toolbar/top-pill-toolbar.component';
import { UiNoticeComponent } from './presentation/components/ui-notice/ui-notice.component';

function isEditableElement(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  const tag = target.tagName.toUpperCase();
  if (tag === 'INPUT' || tag === 'SELECT') {
    return true;
  }
  if (tag === 'TEXTAREA') {
    return !(target as HTMLTextAreaElement).readOnly;
  }
  return Boolean(target.isContentEditable);
}

@Component({
  selector: 'ud-root',
  standalone: true,
  imports: [
    TopPillToolbarComponent,
    UiNoticeComponent,
    CanvasStageComponent,
    ConnectModalComponent,
    ToolsPopoverComponent,
  ],
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <ud-top-pill-toolbar />
    <ud-ui-notice />
    <ud-canvas-stage />
    <ud-connect-modal />
    <ud-tools-popover #toolsPopover />
  `,
})
export class AppComponent implements OnInit, OnDestroy {
  @ViewChild('toolsPopover')
  private readonly toolsPopover?: ToolsPopoverComponent;

  protected readonly facade = inject(UltraDeskSessionFacade);
  private lastCtrlCSent = 0;

  private readonly keyDownHandler = (event: KeyboardEvent): void => {
    if (event.key === 'Escape' && this.facade.store.overlay() !== 'none') {
      event.preventDefault();
      this.facade.setOverlay('none');
      return;
    }
    if (isEditableElement(event.target)) {
      return;
    }
    if (event.key === 'F9' && !event.ctrlKey && !event.altKey && !event.metaKey) {
      event.preventDefault();
      this.facade.cycleRemoteMonitor();
      return;
    }
    const vk = resolveVirtualKeyCode(event);
    if (vk === null) {
      return;
    }
    event.preventDefault();
    if (event.ctrlKey && vk !== 17 && !this.facade.isKeyHeld(17)) {
      this.facade.pressKey(17);
    }
    if (event.shiftKey && vk !== 16 && !this.facade.isKeyHeld(16)) {
      this.facade.pressKey(16);
    }
    if (event.altKey && vk !== 18 && !this.facade.isKeyHeld(18)) {
      this.facade.pressKey(18);
    }
    if ((vk === 16 || vk === 17 || vk === 18) && this.facade.isKeyHeld(vk)) {
      return;
    }
    if (event.repeat && (event.ctrlKey || event.altKey || event.metaKey)) {
      return;
    }
    if (vk === 67 && (event.ctrlKey || this.facade.isKeyHeld(17))) {
      this.lastCtrlCSent = performance.now();
    }
    this.facade.pressKey(vk);
  };

  private readonly keyUpHandler = (event: KeyboardEvent): void => {
    if (isEditableElement(event.target) && !this.facade.hasAnyHeldKeys()) {
      return;
    }
    const vk = resolveVirtualKeyCode(event);
    if (vk !== null) {
      event.preventDefault();
      this.facade.releaseKey(vk);
    }
    if (!event.ctrlKey && vk !== 17 && this.facade.isKeyHeld(17)) {
      this.facade.releaseKey(17);
    }
    if (!event.shiftKey && vk !== 16 && this.facade.isKeyHeld(16)) {
      this.facade.releaseKey(16);
    }
    if (!event.altKey && vk !== 18 && this.facade.isKeyHeld(18)) {
      this.facade.releaseKey(18);
    }
  };

  private readonly copyHandler = (event: ClipboardEvent): void => {
    if (isEditableElement(event.target)) {
      return;
    }
    const sel = typeof window.getSelection === 'function' ? String(window.getSelection()) : '';
    if (!sel) {
      event.preventDefault();
      if (performance.now() - this.lastCtrlCSent > 150) {
        this.lastCtrlCSent = performance.now();
        this.facade.sendInput({ type: 'key', vk: 17, down: true });
        this.facade.sendInput({ type: 'key', vk: 67, down: true });
        this.facade.sendInput({ type: 'key', vk: 67, down: false });
        this.facade.sendInput({ type: 'key', vk: 17, down: false });
      }
    }
  };

  private readonly blurHandler = (): void => {
    this.facade.releaseAllInputs();
  };

  private readonly resizeHandler = (): void => {
    if (this.toolsPopover) {
      this.toolsPopover.positionToolsPopover();
      if (this.facade.store.overlay() === 'tools') {
        this.toolsPopover.syncTabIndicator(false);
      }
    }
  };

  ngOnInit(): void {
    this.facade.initialize();
    window.addEventListener('keydown', this.keyDownHandler, true);
    window.addEventListener('keyup', this.keyUpHandler, true);
    window.addEventListener('copy', this.copyHandler, true);
    window.addEventListener('blur', this.blurHandler);
    window.addEventListener('resize', this.resizeHandler);
  }

  ngOnDestroy(): void {
    window.removeEventListener('keydown', this.keyDownHandler, true);
    window.removeEventListener('keyup', this.keyUpHandler, true);
    window.removeEventListener('copy', this.copyHandler, true);
    window.removeEventListener('blur', this.blurHandler);
    window.removeEventListener('resize', this.resizeHandler);
  }
}
