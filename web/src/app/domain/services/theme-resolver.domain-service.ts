import {
  LIGHT_PALETTES,
  ThemeMode,
  ThemePalette,
  VALID_PALETTES,
  VALID_THEME_MODES,
} from '../models/preferences.model';

export function isValidThemeMode(value: unknown): value is ThemeMode {
  return typeof value === 'string' && (VALID_THEME_MODES as readonly string[]).includes(value);
}

export function isValidPalette(value: unknown): value is ThemePalette {
  return typeof value === 'string' && (VALID_PALETTES as readonly string[]).includes(value);
}

export function resolveActiveTheme(
  mode: ThemeMode,
  hourOverride: number | null = null,
  osPrefersLight = false,
): ThemePalette {
  if (mode === 'auto-time') {
    const h = hourOverride !== null ? Number(hourOverride) : new Date().getHours();
    return h >= 6 && h < 18 ? 'vscode-light' : 'onedark';
  }
  if (mode === 'auto-os') {
    return osPrefersLight ? 'vscode-light' : 'onedark';
  }
  return isValidPalette(mode) ? mode : 'onedark';
}

export function computeNextThemeMode(
  currentMode: ThemeMode,
  activePalette: ThemePalette,
  shiftKey: boolean,
): ThemeMode {
  if (shiftKey) {
    const isLight = LIGHT_PALETTES.includes(activePalette);
    return isLight ? 'onedark' : 'vscode-light';
  }
  const idx = VALID_THEME_MODES.indexOf(currentMode);
  const nextIdx = (idx + 1) % VALID_THEME_MODES.length;
  return VALID_THEME_MODES[nextIdx] ?? 'auto-time';
}
