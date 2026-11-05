import json
import os
from pathlib import Path
import sys
import urllib.request

os.environ.setdefault("WEBKIT_DISABLE_DMABUF_RENDERER", "1")
os.environ.setdefault("WEBKIT_DISABLE_COMPOSITING_MODE", "1")

try:
    import gi

    gi.require_version("Gtk", "3.0")
    gi.require_version("Gdk", "3.0")
    _webkit_loaded = False
    for _webkit_ver in ("4.1", "4.0"):
        try:
            gi.require_version("WebKit2", _webkit_ver)
            _webkit_loaded = True
            break
        except ValueError:
            continue
    if not _webkit_loaded:
        sys.stderr.write("Neither WebKit2 4.1 nor WebKit2 4.0 is installed.\n")
        sys.exit(2)
    from gi.repository import Gdk, GLib, Gtk, WebKit2
except Exception as exc:
    sys.stderr.write(f"GTK/WebKit2 initialization failed: {exc}\n")
    sys.exit(2)


def _norm_clip(text):
    if not text:
        return ""
    return text.replace("\r\n", "\n").rstrip("\r\n")


def main(port=8765):
    if len(sys.argv) > 1 and sys.argv[1].isdigit():
        port = int(sys.argv[1])
    url = f"http://127.0.0.1:{port}"

    GLib.set_prgname("ultradesk-rust")
    Gtk.init()
    win = Gtk.Window(title="UltraDesk (Rust)")
    win.set_default_size(1100, 800)
    win.set_position(Gtk.WindowPosition.CENTER)
    win.connect("destroy", Gtk.main_quit)

    icon_path = Path(__file__).with_name("ultradesk-rust.png")
    if not icon_path.exists():
        icon_path = Path("/usr/share/icons/hicolor/512x512/apps/ultradesk-rust.png")
    if icon_path.exists():
        try:
            win.set_icon_from_file(str(icon_path))
        except Exception:
            pass

    webview = WebKit2.WebView()
    settings = webview.get_settings()
    settings.set_enable_javascript(True)
    settings.set_enable_webgl(False)
    try:
        settings.set_hardware_acceleration_policy(
            WebKit2.HardwareAccelerationPolicy.NEVER
        )
    except Exception:
        pass

    clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    clip_state = {"remote": "", "host": "", "seeded": False, "connected": False}
    viewer_file = Path(__file__).with_name("viewer.html")
    viewer_state = {
        "mtime": viewer_file.stat().st_mtime if viewer_file.exists() else 0.0
    }

    def on_gtk_text(_clip, text):
        if not text:
            return
        if not clip_state["seeded"]:
            clip_state["seeded"] = True
            clip_state["host"] = text
            return
        if not clip_state["connected"]:
            clip_state["host"] = text
            return
        if _norm_clip(text) != _norm_clip(clip_state["host"]) and _norm_clip(
            text
        ) != _norm_clip(clip_state["remote"]):
            clip_state["host"] = text
            try:
                body = json.dumps(
                    {"text": text, "sync_from_system": False}
                ).encode("utf-8")
                req = urllib.request.Request(
                    f"{url}/clipboard",
                    data=body,
                    headers={"Content-Type": "application/json"},
                    method="POST",
                )
                urllib.request.urlopen(req, timeout=0.25).close()
            except Exception:
                pass

    def sync_gtk_clipboard():
        try:
            if viewer_file.exists():
                mt = viewer_file.stat().st_mtime
                if viewer_state["mtime"] and mt > viewer_state["mtime"]:
                    viewer_state["mtime"] = mt
                    webview.reload_bypass_cache()
                else:
                    viewer_state["mtime"] = mt
        except Exception:
            pass
        try:
            with urllib.request.urlopen(f"{url}/status", timeout=0.25) as resp:
                status = json.loads(resp.read().decode("utf-8", errors="replace"))
            clip_state["connected"] = status.get("state") == "connected"
            remote_text = status.get("clipboard_remote") or ""
            if remote_text and _norm_clip(remote_text) != _norm_clip(
                clip_state["remote"]
            ):
                clip_state["remote"] = remote_text
                clip_state["host"] = remote_text
                clipboard.set_text(remote_text, -1)
                clipboard.store()
        except Exception:
            pass
        try:
            clipboard.request_text(on_gtk_text)
        except Exception:
            pass
        return True

    GLib.timeout_add(700, sync_gtk_clipboard)

    webview.load_uri(url)
    win.add(webview)
    win.show_all()
    Gtk.main()


if __name__ == "__main__":
    main()
