import { MonitorRect } from '../models/session.model';

export function normalizeSelectedIndices(
  monitors: readonly MonitorRect[],
  selectedMonitor: number | null | undefined,
  selectedMonitors: readonly number[] | null | undefined,
): number[] {
  const total = Array.isArray(monitors) ? monitors.length : 0;
  if (total === 0) {
    return [];
  }
  if (Array.isArray(selectedMonitors) && selectedMonitors.length > 0) {
    const valid = selectedMonitors
      .map(Number)
      .filter((i) => Number.isInteger(i) && i >= 0 && i < total);
    if (valid.length > 0) {
      return Array.from(new Set(valid)).sort((a, b) => a - b);
    }
  }
  if (
    selectedMonitor !== null &&
    selectedMonitor !== undefined &&
    Number.isInteger(Number(selectedMonitor))
  ) {
    const idx = Number(selectedMonitor);
    if (idx >= 0 && idx < total) {
      return [idx];
    }
  }
  return Array.from({ length: total }, (_, i) => i);
}

export function toggleMonitorSelection(
  total: number,
  currentSelected: readonly number[],
  idx: number,
  forceToggle: boolean,
): number[] | null {
  if (total <= 1) {
    return null;
  }
  const allSelected = currentSelected.length === 0 || currentSelected.length >= total;
  if (!forceToggle) {
    if (!allSelected && currentSelected.length === 1 && currentSelected[0] === idx) {
      return null;
    }
    return [idx];
  }
  let next = allSelected ? [idx] : currentSelected.slice();
  if (!allSelected) {
    if (next.includes(idx)) {
      next = next.filter((x) => x !== idx);
    } else {
      next.push(idx);
    }
  }
  next = Array.from(new Set(next)).sort((a, b) => a - b);
  if (next.length === 0 || next.length >= total) {
    return null;
  }
  return next;
}

export function computeNextCycledMonitorSelection(
  monitors: readonly MonitorRect[],
  selectedMonitor: number | null | undefined,
  selectedMonitors: readonly number[] | null | undefined,
): number[] | null | undefined {
  if (!Array.isArray(monitors) || monitors.length <= 1) {
    return undefined;
  }
  const total = monitors.length;
  const sel = normalizeSelectedIndices(monitors, selectedMonitor, selectedMonitors);
  if (sel.length >= total) {
    return [0];
  }
  if (sel.length === 1 && sel[0] + 1 < total) {
    return [sel[0] + 1];
  }
  return null;
}

export interface MonitorViewItem {
  readonly index: number;
  readonly width: number;
  readonly height: number;
  readonly checked: boolean;
  readonly active: boolean;
}

export function buildMonitorViewItems(
  monitors: readonly MonitorRect[],
  selectedMonitor: number | null | undefined,
  selectedMonitors: readonly number[] | null | undefined,
): {
  readonly allActive: boolean;
  readonly selected: readonly number[];
  readonly items: readonly MonitorViewItem[];
} {
  const total = monitors.length;
  const sel = normalizeSelectedIndices(monitors, selectedMonitor, selectedMonitors);
  const allActive = total === 0 || sel.length >= total;
  const items: MonitorViewItem[] = monitors.map((m, i) => {
    const width = m[2] - m[0];
    const height = m[3] - m[1];
    const checked = sel.includes(i);
    const active = !allActive && checked;
    return { index: i, width, height, checked, active };
  });
  return { allActive, selected: sel, items };
}
