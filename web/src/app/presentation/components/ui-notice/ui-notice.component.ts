import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { UltraDeskSessionFacade } from '../../../application/facade/ultradesk-session.facade';

@Component({
  selector: 'ud-ui-notice',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <aside
      id="uiNotice"
      class="ui-notice"
      [class.active]="store.notice() !== null"
      [class.notice-error]="store.notice()?.kind === 'error'"
      [class.notice-warn]="store.notice()?.kind === 'warn'"
      [class.notice-info]="store.notice()?.kind === 'info'"
      role="alert"
      aria-live="assertive"
    >
      @if (store.notice(); as activeNotice) {
        <div class="ui-notice-content">
          <span class="ui-notice-text">{{ activeNotice.message }}</span>
        </div>
        <button
          type="button"
          class="ui-notice-close"
          aria-label="Close"
          (click)="facade.hideNotice()"
        >
          &times;
        </button>
      }
    </aside>
  `,
})
export class UiNoticeComponent {
  protected readonly facade = inject(UltraDeskSessionFacade);
  protected readonly store = this.facade.store;
}
