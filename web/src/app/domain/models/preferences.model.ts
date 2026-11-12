export const VALID_THEME_MODES = [
  'auto-time',
  'onedark',
  'vscode-dark',
  'vscode-light',
  'onelight',
  'gruvbox-dark',
  'gruvbox-light',
  'auto-os',
] as const;

export type ThemeMode = (typeof VALID_THEME_MODES)[number];

export const VALID_PALETTES = [
  'onedark',
  'onelight',
  'vscode-dark',
  'vscode-light',
  'gruvbox-dark',
  'gruvbox-light',
] as const;

export type ThemePalette = (typeof VALID_PALETTES)[number];

export const LIGHT_PALETTES: readonly ThemePalette[] = [
  'vscode-light',
  'onelight',
  'gruvbox-light',
];

export const THEME_SHORT_LABELS: Readonly<Record<ThemeMode, string>> = {
  'auto-time': 'Auto',
  'auto-os': 'OS',
  onedark: 'OneDark',
  onelight: 'OneLight',
  'vscode-dark': 'VS Dark',
  'vscode-light': 'VS Light',
  'gruvbox-dark': 'Gruvbox',
  'gruvbox-light': 'GruvLight',
};

export type LanguageCode = 'vi' | 'en';
export type OverlayKind = 'none' | 'connect' | 'tools' | 'update';
export type ToolTab = 'chat' | 'files' | 'clipboard' | 'audio';
export type NoticeKind = 'error' | 'warn' | 'info';

export interface NoticeBanner {
  readonly message: string;
  readonly kind: NoticeKind;
}

export interface SavedCredentialsLocal {
  readonly id: string;
  readonly remember: boolean;
  readonly alias?: string;
}
