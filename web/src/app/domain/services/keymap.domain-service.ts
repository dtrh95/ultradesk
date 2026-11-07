import { CanvasProjection, MouseButton, Point2D } from '../models/input.model';

const SPECIAL_KEY_MAP: Readonly<Record<string, number>> = {
  Enter: 13,
  Escape: 27,
  Backspace: 8,
  Tab: 9,
  Space: 32,
  ShiftLeft: 16,
  ShiftRight: 16,
  ControlLeft: 17,
  ControlRight: 17,
  AltLeft: 18,
  AltRight: 18,
  MetaLeft: 91,
  MetaRight: 92,
  OSLeft: 91,
  OSRight: 92,
  ArrowLeft: 37,
  ArrowUp: 38,
  ArrowRight: 39,
  ArrowDown: 40,
  Home: 36,
  End: 35,
  PageUp: 33,
  PageDown: 34,
  Delete: 46,
  Insert: 45,
  CapsLock: 20,
  Semicolon: 186,
  Equal: 187,
  Comma: 188,
  Minus: 189,
  Period: 190,
  Slash: 191,
  Backquote: 192,
  BracketLeft: 219,
  Backslash: 220,
  BracketRight: 221,
  Quote: 222,
};

export function normalizeVirtualKey(vk: number): number {
  if (vk === 160 || vk === 161) return 16;
  if (vk === 162 || vk === 163) return 17;
  if (vk === 164 || vk === 165) return 18;
  return vk;
}

export function resolveVirtualKeyCode(event: {
  readonly code?: string;
  readonly key?: string;
  readonly keyCode?: number;
  readonly which?: number;
}): number | null {
  const code = event.code ?? '';
  const mappedSpecial = SPECIAL_KEY_MAP[code];
  if (mappedSpecial !== undefined) {
    return mappedSpecial;
  }
  const k = event.key ?? '';
  if (k === 'Control') return 17;
  if (k === 'Shift') return 16;
  if (k === 'Alt') return 18;
  if (k === 'Meta' || k === 'Super' || k === 'OS') return 91;
  if (/^Key[A-Z]$/.test(code)) return code.charCodeAt(3);
  if (/^Digit[0-9]$/.test(code)) return code.charCodeAt(5);
  if (/^F([1-9]|1[0-2])$/.test(code)) return 111 + Number(code.slice(1));
  if (/^[a-zA-Z]$/.test(k)) return k.toUpperCase().charCodeAt(0);
  if (/^[0-9]$/.test(k)) return k.charCodeAt(0);
  const raw = Number(event.keyCode ?? event.which ?? 0);
  if (raw > 0 && raw <= 255) {
    return normalizeVirtualKey(raw);
  }
  return null;
}

export function resolveMouseButton(buttonIndex: number): MouseButton | null {
  if (buttonIndex === 0) return 'left';
  if (buttonIndex === 1) return 'middle';
  if (buttonIndex === 2) return 'right';
  return null;
}

export function computeCanvasContentRect(
  rect: {
    readonly left: number;
    readonly top: number;
    readonly width: number;
    readonly height: number;
  },
  screenWidth: number,
  screenHeight: number,
): CanvasProjection {
  const sw = screenWidth || 1;
  const sh = screenHeight || 1;
  const rw = rect.width || sw;
  const rh = rect.height || sh;
  const scale = Math.min(rw / sw, rh / sh) || 1;
  const drawW = sw * scale;
  const drawH = sh * scale;
  const left = rect.left + (rw - drawW) / 2;
  const top = rect.top + (rh - drawH) / 2;
  return { left, top, width: drawW, height: drawH, scale, sw, sh };
}

export function projectClientPointToCanvas(
  clientX: number,
  clientY: number,
  projection: CanvasProjection,
): Point2D {
  const x = Math.max(
    0,
    Math.min(projection.sw - 1, Math.round((clientX - projection.left) / projection.scale)),
  );
  const y = Math.max(
    0,
    Math.min(projection.sh - 1, Math.round((clientY - projection.top) / projection.scale)),
  );
  return { x, y };
}
