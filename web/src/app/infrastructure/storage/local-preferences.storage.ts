import { Injectable } from '@angular/core';
import {
  LanguageCode,
  SavedCredentialsLocal,
  ThemeMode,
} from '../../domain/models/preferences.model';
import { SavedRemote } from '../../domain/models/session.model';
import { isValidThemeMode } from '../../domain/services/theme-resolver.domain-service';

export interface HydratedPreferences {
  readonly lang: LanguageCode;
  readonly themeMode: ThemeMode;
  readonly hourOverride: number | null;
  readonly pillCollapsed: boolean;
  readonly savedRemotes: readonly SavedRemote[];
  readonly savedCreds: SavedCredentialsLocal | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

@Injectable({ providedIn: 'root' })
export class LocalPreferencesStorage {
  hydrateFromUrlAndStorage(): HydratedPreferences {
    let lang: LanguageCode = 'en';
    let themeMode: ThemeMode = 'auto-time';
    let hourOverride: number | null = null;
    let pillCollapsed = false;
    let savedRemotes: SavedRemote[] = [];
    let savedCreds: SavedCredentialsLocal | null = null;

    try {
      const params = new URLSearchParams(window.location.search);
      const qLang = params.get('lang');
      const storedLang = qLang || localStorage.getItem('uv_lang');
      if (storedLang === 'en' || storedLang === 'vi') {
        lang = storedLang;
      }

      const qTheme = params.get('theme');
      const storedTheme = qTheme || localStorage.getItem('uv_theme');
      if (isValidThemeMode(storedTheme)) {
        themeMode = storedTheme;
      }

      const qHour = params.get('hour');
      const parsedHour = qHour !== null && qHour !== '' ? Number(qHour) : null;
      if (parsedHour !== null && Number.isFinite(parsedHour)) {
        hourOverride = parsedHour;
      }

      const qPill = params.get('pill');
      const storedPill = localStorage.getItem('uv_pill_collapsed');
      if (qPill === 'collapsed' || qPill === '1') {
        pillCollapsed = true;
      } else if (qPill === 'expanded' || qPill === '0') {
        pillCollapsed = false;
      } else if (storedPill === '1') {
        pillCollapsed = true;
      }

      const rawList: unknown = JSON.parse(localStorage.getItem('uv_saved_remotes_list') || '[]');
      if (Array.isArray(rawList)) {
        savedRemotes = rawList
          .filter((item): item is Record<string, unknown> => isRecord(item))
          .filter((item) => typeof item['partner_id'] === 'string' && item['partner_id'].length > 0)
          .map((item) => ({
            partner_id: String(item['partner_id']),
            password: '',
          }));
        if (rawList.some((item) => isRecord(item) && 'password' in item)) {
          localStorage.setItem(
            'uv_saved_remotes_list',
            JSON.stringify(savedRemotes.map((r) => ({ partner_id: r.partner_id }))),
          );
        }
      }

      const rawCreds: unknown = JSON.parse(localStorage.getItem('uv_saved_creds') || 'null');
      if (isRecord(rawCreds)) {
        const remember = rawCreds['remember'] !== false;
        savedCreds = {
          id: remember && typeof rawCreds['id'] === 'string' ? rawCreds['id'] : '',
          remember,
        };
        if ('pass' in rawCreds) {
          localStorage.setItem('uv_saved_creds', JSON.stringify(savedCreds));
        }
      }
    } catch {
      void 0;
    }

    return {
      lang,
      themeMode,
      hourOverride,
      pillCollapsed,
      savedRemotes,
      savedCreds,
    };
  }

  saveLanguage(lang: LanguageCode): void {
    try {
      localStorage.setItem('uv_lang', lang);
    } catch {
      void 0;
    }
  }

  saveThemeMode(mode: ThemeMode): void {
    try {
      localStorage.setItem('uv_theme', mode);
    } catch {
      void 0;
    }
  }

  savePillCollapsed(collapsed: boolean): void {
    try {
      localStorage.setItem('uv_pill_collapsed', collapsed ? '1' : '0');
    } catch {
      void 0;
    }
  }

  saveRemotesList(remotes: readonly SavedRemote[]): void {
    try {
      localStorage.setItem(
        'uv_saved_remotes_list',
        JSON.stringify(remotes.map((r) => ({ partner_id: r.partner_id }))),
      );
    } catch {
      void 0;
    }
  }

  saveCredentials(id: string, remember: boolean): void {
    try {
      if (remember) {
        localStorage.setItem('uv_saved_creds', JSON.stringify({ id, remember: true }));
      } else {
        localStorage.setItem('uv_saved_creds', JSON.stringify({ id: '', remember: false }));
        localStorage.removeItem('uv_saved_remotes_list');
      }
    } catch {
      void 0;
    }
  }
}
