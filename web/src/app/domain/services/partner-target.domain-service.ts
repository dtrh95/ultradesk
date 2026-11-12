import type { SavedRemote } from '../models/session.model';

const IPV4_RE = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/;

function isTrimNoise(char: string): boolean {
  if (!char) {
    return false;
  }
  const code = char.charCodeAt(0);
  return (
    code <= 32 ||
    code === 0x00a0 ||
    code === 0x200b ||
    code === 0x200c ||
    code === 0x200d ||
    code === 0xfeff
  );
}

function trimNoise(value: string): string {
  let start = 0;
  while (start < value.length && isTrimNoise(value[start])) {
    start++;
  }
  let end = value.length;
  while (end > start && isTrimNoise(value[end - 1])) {
    end--;
  }
  return value.slice(start, end);
}

function removeAllNoise(value: string): string {
  let result = '';
  for (const ch of value) {
    if (!isTrimNoise(ch)) {
      result += ch;
    }
  }
  return result;
}

function stripPrefix(trimmed: string): string {
  const lower = trimmed.toLowerCase();
  const prefixes = [
    'partner id',
    'partner_id',
    'partner-id',
    'remote id',
    'remote_id',
    'remote-id',
    'id',
  ];
  for (const prefix of prefixes) {
    if (lower.startsWith(prefix)) {
      const rest = trimmed.slice(prefix.length);
      let idx = 0;
      while (idx < rest.length && isTrimNoise(rest[idx])) {
        idx++;
      }
      const sep = rest[idx];
      if (sep === ':' || sep === '=') {
        return trimNoise(rest.slice(idx + 1));
      }
    }
  }
  return trimmed;
}

function isValidIpv4(host: string): boolean {
  const match = IPV4_RE.exec(host);
  if (!match) {
    return false;
  }
  for (let i = 1; i <= 4; i++) {
    const octet = Number(match[i]);
    if (octet < 0 || octet > 255) {
      return false;
    }
  }
  return true;
}

export function normalizePartnerTarget(raw: string): string {
  const trimmed = trimNoise(raw);
  if (!trimmed) {
    return '';
  }

  const withoutPrefix = stripPrefix(trimmed);
  const compact = removeAllNoise(withoutPrefix);
  if (!compact) {
    return '';
  }

  const lower = compact.toLowerCase();
  if (
    lower.startsWith('lan://') ||
    lower.startsWith('tcp://') ||
    lower.startsWith('[') ||
    lower.includes(':') ||
    lower === 'localhost' ||
    isValidIpv4(compact)
  ) {
    return compact;
  }

  const digits = compact.replace(/[-–—−._]/g, '');
  if (digits.length > 0 && /^\d+$/.test(digits)) {
    return digits;
  }

  return compact;
}

export function sanitizePartnerAlias(raw: string): string {
  let cleaned = '';
  for (const ch of raw) {
    const code = ch.charCodeAt(0);
    if (code >= 32 && (code < 127 || code >= 160)) {
      cleaned += ch;
    }
  }
  const trimmed = trimNoise(cleaned);
  return trimNoise(Array.from(trimmed).slice(0, 64).join(''));
}

export function formatPartnerIdForDisplay(partnerId: string): string {
  const digits = partnerId.trim();
  if (/^\d{9}$/.test(digits)) {
    return `${digits.slice(0, 3)} ${digits.slice(3, 6)} ${digits.slice(6, 9)}`;
  }
  return partnerId;
}

export function resolveRemoteDisplayName(
  partnerId: string,
  savedRemotes: readonly SavedRemote[],
  explicitAlias?: string,
): string {
  const cleanExplicit = explicitAlias ? sanitizePartnerAlias(explicitAlias) : '';
  if (cleanExplicit) {
    return cleanExplicit;
  }
  const normalizedId = normalizePartnerTarget(partnerId);
  if (normalizedId) {
    const found = savedRemotes.find((r) => r.partner_id === normalizedId);
    if (found?.alias && found.alias.trim()) {
      return found.alias.trim();
    }
  }
  return formatPartnerIdForDisplay(partnerId);
}
