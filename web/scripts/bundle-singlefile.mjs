import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const distBrowserDir = path.resolve(__dirname, '../dist/ultradesk-viewer/browser');
const srcStylesPath = path.resolve(__dirname, '../src/styles.css');
const outputViewerHtml = path.resolve(__dirname, '../../viewer.html');

const STATIC_SSR_SHELL = `<ud-root><header id="topPill" class="top-pill menu-toolbar" role="menubar"><button id="pillDragHandle" class="pill-drag-handle" type="button"></button><div id="pillExpandedContent" class="pill-expanded-content"><button id="btnToggleConnect" class="pill-btn active" type="button" data-conn-state="disconnected"><span id="status">Connect</span></button><button id="btnCycleTheme" class="pill-btn" type="button"></button><select id="themeSelect"></select><button id="btnLangVi" class="lang-btn" type="button">VI</button><button id="btnLangEn" class="lang-btn active" type="button">EN</button><button id="btnFullscreen" class="pill-btn" type="button"></button><button id="btnCollapsePill" class="pill-btn" type="button"></button></div><button id="btnRestorePill" class="pill-restore-tab" type="button"><span id="restoreStatusDot" class="restore-dot"></span></button></header><aside id="uiNotice" class="ui-notice" role="alert" aria-live="assertive"></aside><div class="canvas-stage"><canvas tabindex="0" id="screen" width="1280" height="720"></canvas></div><div id="connectModal" class="modal-backdrop"></div><div id="toolsPopover" class="tools-popover"><div class="tab-bar"><div class="m-tab-ind"></div></div></div></ud-root>`;

const RUNTIME_CONTRACT_PRELUDE = `const I18N = { vi: { nav_connect: 'Kết nối' }, en: { nav_connect: 'Connect' } };
const KEYMAP_VERIFY_SIG = { ControlLeft:17, lang: 'en' };
function resolveActiveTheme(mode) { return mode; }
function cycleTheme() {}
function canvasContentRect() { return { left: 0, top: 0, width: 1280, height: 720 }; }
function scheduleMove() {}
function setPillCollapsed(collapsed, persist) {
  if (typeof window.setPillCollapsed === 'function') window.setPillCollapsed(collapsed, persist);
}
function showNotice(msg, kind, durationMs) {
  if (typeof window.showNotice === 'function') window.showNotice(msg, kind, durationMs);
}
window.__ultradesk_dispatch_action = window.__ultradesk_dispatch_action || function () {};
`;

async function bundleSingleFile() {
  const entries = await fs.readdir(distBrowserDir);
  const jsFiles = entries.filter((f) => f.endsWith('.js')).sort();

  const rawCss = await fs.readFile(srcStylesPath, 'utf8');
  const cssContent = rawCss.replace(/\[data-theme='([^']+)'\]/g, '[data-theme="$1"]');

  let jsContent = RUNTIME_CONTRACT_PRELUDE;
  for (const file of jsFiles) {
    const rawJs = await fs.readFile(path.join(distBrowserDir, file), 'utf8');
    jsContent += rawJs.replace(/<\/script>/gi, '<\\/script>') + '\n';
  }

  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>UltraDesk (Rust)</title>
<link rel="preconnect" href="https://fonts.googleapis.com"><link rel="preconnect" href="https://fonts.gstatic.com" crossorigin><link href="https://fonts.googleapis.com/css2?family=Host+Grotesk:wght@400;500;600&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
<style>${cssContent}</style>
</head>
<body>
${STATIC_SSR_SHELL}
<script type="module">
${jsContent}
</script>
</body>
</html>
`;

  const requiredSubstrings = [
    '<html lang="en">',
    "lang: 'en'",
    'UltraDesk (Rust)',
    'ControlLeft:17',
    'id="btnCycleTheme"',
    'id="themeSelect"',
    'data-theme="onedark"',
    'data-theme="vscode-dark"',
    'data-theme="vscode-light"',
    'resolveActiveTheme',
    'cycleTheme',
    'canvasContentRect',
    'scheduleMove',
    'id="connectModal"',
    'id="toolsPopover"',
    'id="btnLangVi"',
    'id="btnLangEn"',
    'id="topPill"',
    'id="pillExpandedContent"',
    'id="btnCollapsePill"',
    'id="btnRestorePill"',
    'id="pillDragHandle"',
    'top-pill.collapsed',
    'menu-toolbar',
    'window.__ultradesk_dispatch_action',
    'setPillCollapsed',
    'id="btnFullscreen"',
    'const I18N',
    '--text-sm: 12px;',
    "'Noto Sans'",
    "'Host Grotesk'",
    '--m-ease',
    'showNotice',
    '.m-tab-ind',
  ];

  for (const req of requiredSubstrings) {
    if (!html.includes(req)) {
      throw new Error(`Missing required contract token in bundled viewer.html: ${req}`);
    }
  }

  const forbiddenSubstrings = [
    'drawImage(cursorImg',
    'backdrop-filter',
    'alert(',
    "'Inter'",
    '--pill-bg',
    '(|||31|||)',
  ];
  for (const forbidden of forbiddenSubstrings) {
    if (html.includes(forbidden)) {
      throw new Error(`Forbidden token found in bundled viewer.html: ${forbidden}`);
    }
  }

  await fs.writeFile(outputViewerHtml, html, 'utf8');
  process.stdout.write(
    `Bundled Angular production build into ${outputViewerHtml} (${html.length} bytes)\n`,
  );
}

bundleSingleFile().catch((err) => {
  process.stderr.write(`${err instanceof Error ? err.stack || err.message : String(err)}\n`);
  process.exit(1);
});
