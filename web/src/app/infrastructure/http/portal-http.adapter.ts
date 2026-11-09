import { Injectable } from '@angular/core';
import { RemoteInputEvent } from '../../domain/models/input.model';
import { SessionSnapshot } from '../../domain/models/session.model';
import { parseSessionSnapshot } from '../parsers/session-snapshot.parser';

const VIEWER_TOKEN = '__TOKEN__';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export interface FrameFetchResult {
  readonly kind: 'not-modified' | 'frame';
  readonly version: number;
  readonly blob?: Blob;
}

@Injectable({ providedIn: 'root' })
export class PortalHttpAdapter {
  async fetchStatus(): Promise<SessionSnapshot> {
    const response = await fetch('/status', {
      headers: {
        'X-Viewer-Token': VIEWER_TOKEN,
      },
      signal: AbortSignal.timeout(3000),
    });
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    const raw: unknown = await response.json();
    return parseSessionSnapshot(raw);
  }

  async postJson(
    path: string,
    payload: Readonly<Record<string, unknown>> = {},
    fallbackError = 'Request failed',
  ): Promise<Record<string, unknown>> {
    const response = await fetch(path, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'X-Viewer-Token': VIEWER_TOKEN,
      },
      body: JSON.stringify(payload),
      keepalive: true,
    });
    const data: unknown = await response.json().catch(() => ({}));
    const record = isRecord(data) ? data : {};
    if (!response.ok || record['ok'] === false) {
      const errMsg = typeof record['error'] === 'string' ? record['error'] : fallbackError;
      throw new Error(errMsg);
    }
    return record;
  }

  async checkUpdate(apiUrl?: string): Promise<Record<string, unknown>> {
    const payload = apiUrl ? { api_url: apiUrl } : {};
    return this.postJson('/update/check', payload, 'Failed to check for updates');
  }

  async confirmUpdate(): Promise<Record<string, unknown>> {
    return this.postJson('/update/confirm', {}, 'Failed to confirm update');
  }

  async dismissUpdate(version?: string | null): Promise<Record<string, unknown>> {
    const payload = version ? { version } : {};
    return this.postJson('/update/later', payload, 'Failed to dismiss update');
  }

  async sendInputEvent(event: RemoteInputEvent, fallbackError: string): Promise<void> {
    await this.postJson('/event', event, fallbackError);
  }

  async fetchFrame(
    needNewVersion: boolean,
    lastFrameVersion: number,
    targetQuality: number,
    expectedVersion: number,
  ): Promise<FrameFetchResult | null> {
    const url = needNewVersion
      ? `/frame.jpg?v=${lastFrameVersion}&q=${targetQuality}`
      : `/frame.jpg?q=${targetQuality}`;
    const response = await fetch(url, {
      cache: 'no-store',
      headers: {
        'X-Viewer-Token': VIEWER_TOKEN,
      },
      signal: AbortSignal.timeout(3000),
    });
    if (response.status === 304) {
      return { kind: 'not-modified', version: expectedVersion };
    }
    if (response.status === 200) {
      const blob = await response.blob();
      const headerVer = Number(response.headers.get('X-Frame-Version') || expectedVersion);
      return { kind: 'frame', version: headerVer, blob };
    }
    return null;
  }
}
