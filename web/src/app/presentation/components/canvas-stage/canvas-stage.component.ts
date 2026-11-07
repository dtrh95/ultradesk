import {
  AfterViewInit,
  ChangeDetectionStrategy,
  Component,
  ElementRef,
  inject,
  OnDestroy,
  ViewChild,
} from '@angular/core';
import { UltraDeskSessionFacade } from '../../../application/facade/ultradesk-session.facade';
import { Point2D } from '../../../domain/models/input.model';
import {
  computeCanvasContentRect,
  projectClientPointToCanvas,
  resolveMouseButton,
} from '../../../domain/services/keymap.domain-service';

@Component({
  selector: 'ud-canvas-stage',
  standalone: true,
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div class="canvas-stage">
      <canvas
        #screenCanvas
        tabindex="0"
        id="screen"
        width="1280"
        height="720"
        (pointermove)="onPointerMove($event)"
        (pointerdown)="onPointerDown($event)"
        (pointerup)="onPointerUp($event)"
        (contextmenu)="onContextMenu($event)"
        (blur)="facade.releaseAllInputs()"
      ></canvas>
    </div>
  `,
})
export class CanvasStageComponent implements AfterViewInit, OnDestroy {
  @ViewChild('screenCanvas', { static: true })
  private readonly screenCanvasRef!: ElementRef<HTMLCanvasElement>;

  protected readonly facade = inject(UltraDeskSessionFacade);

  private pendingMove: Point2D | null = null;
  private moveTimer: ReturnType<typeof setTimeout> | null = null;
  private moveInFlight = false;
  private lastMove = 0;
  private lastPtX = -1;
  private lastPtY = -1;

  private readonly wheelListener = (event: WheelEvent): void => {
    event.preventDefault();
    this.facade.sendInput({
      type: 'wheel',
      delta: event.deltaY > 0 ? -120 : 120,
    });
  };

  ngAfterViewInit(): void {
    const canvas = this.screenCanvasRef.nativeElement;
    this.facade.registerCanvas(canvas);
    canvas.addEventListener('wheel', this.wheelListener, { passive: false });
  }

  ngOnDestroy(): void {
    const canvas = this.screenCanvasRef.nativeElement;
    canvas.removeEventListener('wheel', this.wheelListener);
    if (this.moveTimer) {
      clearTimeout(this.moveTimer);
      this.moveTimer = null;
    }
  }

  protected onPointerMove(event: PointerEvent): void {
    this.pendingMove = this.pointFromEvent(event);
    this.scheduleMove();
  }

  protected onPointerDown(event: PointerEvent): void {
    event.preventDefault();
    const canvas = this.screenCanvasRef.nativeElement;
    canvas.focus();
    try {
      canvas.setPointerCapture(event.pointerId);
    } catch {
      void 0;
    }
    const button = resolveMouseButton(event.button);
    if (button) {
      if (this.moveTimer) {
        clearTimeout(this.moveTimer);
        this.moveTimer = null;
      }
      this.pendingMove = null;
      const pt = this.pointFromEvent(event);
      this.lastMove = performance.now();
      this.lastPtX = pt.x;
      this.lastPtY = pt.y;
      this.facade.sendInput({ type: 'move', x: pt.x, y: pt.y });
      this.facade.sendInput({ type: 'button', button, down: true });
      this.facade.trackButtonDown(button);
    }
  }

  protected onPointerUp(event: PointerEvent): void {
    const canvas = this.screenCanvasRef.nativeElement;
    try {
      canvas.releasePointerCapture(event.pointerId);
    } catch {
      void 0;
    }
    const button = resolveMouseButton(event.button);
    if (button) {
      this.facade.sendInput({ type: 'button', button, down: false });
      this.facade.trackButtonUp(button);
    }
  }

  protected onContextMenu(event: MouseEvent): void {
    event.preventDefault();
  }

  private pointFromEvent(event: PointerEvent): Point2D {
    const canvas = this.screenCanvasRef.nativeElement;
    const rect = canvas.getBoundingClientRect();
    const projection = computeCanvasContentRect(rect, canvas.width, canvas.height);
    return projectClientPointToCanvas(event.clientX, event.clientY, projection);
  }

  private flushMove(): void {
    if (this.moveTimer) {
      clearTimeout(this.moveTimer);
      this.moveTimer = null;
    }
    if (!this.pendingMove || this.moveInFlight) {
      return;
    }
    const pt = this.pendingMove;
    this.pendingMove = null;
    if (pt.x === this.lastPtX && pt.y === this.lastPtY) {
      return;
    }
    this.lastMove = performance.now();
    this.lastPtX = pt.x;
    this.lastPtY = pt.y;
    const handledViaIpc = this.facade.sendMoveEvent(pt.x, pt.y, () => {
      this.moveInFlight = false;
      if (this.pendingMove) {
        this.scheduleMove();
      }
    });
    if (!handledViaIpc) {
      this.moveInFlight = true;
    }
  }

  private scheduleMove(): void {
    if (!this.pendingMove || this.moveInFlight) {
      return;
    }
    const elapsed = performance.now() - this.lastMove;
    if (elapsed >= 16) {
      this.flushMove();
    } else if (!this.moveTimer) {
      this.moveTimer = setTimeout(() => this.flushMove(), Math.max(4, Math.ceil(16 - elapsed)));
    }
  }
}
