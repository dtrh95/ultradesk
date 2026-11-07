import { LanguageCode, ThemeMode, ToolTab } from './preferences.model';

export type SpecialKeyAction = 'super' | 'ctrl_alt_del' | 'f9_next_monitor';

export type HostDispatchedAction =
  | { readonly action: 'ToggleConnectModal' }
  | { readonly action: 'DisconnectRemote' }
  | { readonly action: 'StartHostSharing' }
  | { readonly action: 'StopHostSharing' }
  | { readonly action: 'ToggleToolsPopover' }
  | { readonly action: 'SelectToolTab'; readonly payload: ToolTab }
  | { readonly action: 'SetTheme'; readonly payload: ThemeMode }
  | { readonly action: 'CycleTheme' }
  | { readonly action: 'SetLanguage'; readonly payload: LanguageCode }
  | { readonly action: 'SelectMonitor'; readonly payload: number | null }
  | { readonly action: 'SendSpecialKey'; readonly payload: SpecialKeyAction }
  | { readonly action: 'ToggleToolbarCollapse' }
  | { readonly action: 'SetToolbarCollapsed'; readonly payload: boolean }
  | { readonly action: 'ToggleFullscreen' }
  | { readonly action: 'CopyHostId' };
