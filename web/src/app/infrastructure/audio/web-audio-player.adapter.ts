import { Injectable } from '@angular/core';

interface WebkitWindow {
  AudioContext?: typeof AudioContext;
  webkitAudioContext?: typeof AudioContext;
}

@Injectable({ providedIn: 'root' })
export class WebAudioPlayerAdapter {
  private audioCtx: AudioContext | null = null;
  private lastAudioSeq = 0;

  async ensureResumed(): Promise<void> {
    const ctx = this.getOrCreateContext();
    if (ctx && ctx.state === 'suspended') {
      await ctx.resume();
    }
  }

  async pollAndPlayAudio(chunks: number, enabled: boolean): Promise<void> {
    if (!enabled || !chunks) {
      return;
    }
    try {
      const response = await fetch(`/audio.wav?after=${this.lastAudioSeq}`);
      if (!response.ok) {
        return;
      }
      const seq = Number(response.headers.get('X-Audio-Seq') || '0');
      if (seq <= this.lastAudioSeq) {
        return;
      }
      this.lastAudioSeq = seq;
      const buf = await response.arrayBuffer();
      if (buf.byteLength <= 44) {
        return;
      }
      const ctx = this.getOrCreateContext();
      if (!ctx || ctx.state === 'suspended') {
        return;
      }
      const audioBuf = await ctx.decodeAudioData(buf);
      const src = ctx.createBufferSource();
      src.buffer = audioBuf;
      src.connect(ctx.destination);
      src.start();
    } catch {
      void 0;
    }
  }

  private getOrCreateContext(): AudioContext | null {
    if (!this.audioCtx) {
      const w = window as unknown as WebkitWindow;
      const Ctor = w.AudioContext ?? w.webkitAudioContext;
      if (Ctor) {
        this.audioCtx = new Ctor();
      }
    }
    return this.audioCtx;
  }
}
