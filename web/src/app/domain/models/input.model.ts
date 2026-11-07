export type MouseButton = 'left' | 'middle' | 'right';

export type RemoteInputEvent =
  | { readonly type: 'move'; readonly x: number; readonly y: number }
  | { readonly type: 'button'; readonly button: MouseButton; readonly down: boolean }
  | { readonly type: 'wheel'; readonly delta: number }
  | { readonly type: 'key'; readonly vk: number; readonly down: boolean };

export interface CanvasProjection {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
  readonly scale: number;
  readonly sw: number;
  readonly sh: number;
}

export interface Point2D {
  readonly x: number;
  readonly y: number;
}
