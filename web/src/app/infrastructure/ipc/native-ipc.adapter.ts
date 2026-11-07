import { Injectable } from '@angular/core';
import { NoticeKind } from '../../domain/models/preferences.model';

interface WryIpcWindow {
  ipc?: {
    postMessage?: (message: string) => void;
  };
  __ultradesk_dispatch_action?: (action: unknown, payload?: unknown) => void;
  setPillCollapsed?: (collapsed: boolean, persist?: boolean) => void;
  showNotice?: (msg: unknown, kind?: NoticeKind, durationMs?: number) => void;
  onRemoteMonClick?: (idx: number, event?: MouseEvent) => void;
  onRemoteMonCheck?: (idx: number, event?: MouseEvent) => void;
  onHostMonClick?: (idx: number, event?: MouseEvent) => void;
  onHostMonCheck?: (idx: number, event?: MouseEvent) => void;
  acceptTransfer?: (fileId: string) => void;
  cancelTransfer?: (fileId: string) => void;
}

function getIpcWindow(): WryIpcWindow {
  return window as unknown as WryIpcWindow;
}

@Injectable({ providedIn: 'root' })
export class NativeIpcAdapter {
  private dispatchingFromHost = false;

  hasNativeIpc(): boolean {
    const w = getIpcWindow();
    return Boolean(w.ipc && typeof w.ipc.postMessage === 'function');
  }

  isDispatchingFromHost(): boolean {
    return this.dispatchingFromHost;
  }

  postMessage(payload: Readonly<Record<string, unknown>>): boolean {
    const w = getIpcWindow();
    if (w.ipc && typeof w.ipc.postMessage === 'function') {
      try {
        w.ipc.postMessage(JSON.stringify(payload));
        return true;
      } catch {
        return false;
      }
    }
    return false;
  }

  notifyHostIfNotDispatching(action: string, payload?: unknown): void {
    if (this.dispatchingFromHost) {
      return;
    }
    const msg: Record<string, unknown> = { action };
    if (payload !== undefined) {
      msg['payload'] = payload;
    }
    this.postMessage(msg);
  }

  registerGlobalBridge(handlers: {
    readonly onDispatchAction: (action: string, payload: unknown) => void;
    readonly onSetPillCollapsed: (collapsed: boolean, persist?: boolean) => void;
    readonly onShowNotice: (msg: unknown, kind?: NoticeKind, durationMs?: number) => void;
    readonly onRemoteMonClick: (idx: number, event?: MouseEvent) => void;
    readonly onRemoteMonCheck: (idx: number, event?: MouseEvent) => void;
    readonly onHostMonClick: (idx: number, event?: MouseEvent) => void;
    readonly onHostMonCheck: (idx: number, event?: MouseEvent) => void;
    readonly onAcceptTransfer: (fileId: string) => void;
    readonly onCancelTransfer: (fileId: string) => void;
  }): void {
    const w = getIpcWindow();
    w.__ultradesk_dispatch_action = (rawAction: unknown, rawPayload?: unknown): void => {
      try {
        this.dispatchingFromHost = true;
        let actionName = '';
        let payloadVal: unknown = rawPayload;
        if (typeof rawAction === 'object' && rawAction !== null) {
          const rec = rawAction as Record<string, unknown>;
          actionName = typeof rec['action'] === 'string' ? rec['action'] : '';
          payloadVal = rec['payload'];
        } else if (typeof rawAction === 'string') {
          actionName = rawAction;
        }
        if (actionName) {
          handlers.onDispatchAction(actionName, payloadVal);
        }
      } finally {
        this.dispatchingFromHost = false;
      }
    };
    w.setPillCollapsed = handlers.onSetPillCollapsed;
    w.showNotice = handlers.onShowNotice;
    w.onRemoteMonClick = handlers.onRemoteMonClick;
    w.onRemoteMonCheck = handlers.onRemoteMonCheck;
    w.onHostMonClick = handlers.onHostMonClick;
    w.onHostMonCheck = handlers.onHostMonCheck;
    w.acceptTransfer = handlers.onAcceptTransfer;
    w.cancelTransfer = handlers.onCancelTransfer;
  }
}
