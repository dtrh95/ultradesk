#!/usr/bin/env bash
set -euo pipefail

if [ "${#}" -lt 1 ]; then
  echo "Usage: $0 <semver> [summary]" >&2
  exit 1
fi

RAW_VER="${1#v}"
SUMMARY="${2:-Release UltraDesk (Rust Edition) ${RAW_VER}}"

if ! [[ "${RAW_VER}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Error: invalid semantic version '${RAW_VER}' (expected MAJOR.MINOR.PATCH)" >&2
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

python3 - "${RAW_VER}" "${SUMMARY}" <<'PY'
import datetime
import json
import pathlib
import re
import sys

new_ver = sys.argv[1]
summary = sys.argv[2]
root = pathlib.Path(".")

cargo_toml_path = root / "Cargo.toml"
cargo_toml = cargo_toml_path.read_text(encoding="utf-8")
m = re.search(r'^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"', cargo_toml, flags=re.M)
if not m:
    raise SystemExit("Could not locate version in Cargo.toml")
old_ver = m.group(1)

cargo_toml = re.sub(
    r'^(version\s*=\s*")[0-9]+\.[0-9]+\.[0-9]+(")',
    rf'\g<1>{new_ver}\g<2>',
    cargo_toml,
    count=1,
    flags=re.M,
)
cargo_toml_path.write_text(cargo_toml, encoding="utf-8")

cargo_lock_path = root / "Cargo.lock"
cargo_lock = cargo_lock_path.read_text(encoding="utf-8")
cargo_lock = re.sub(
    r'(\[\[package\]\]\nname = "ultradesk-rust"\nversion = ")[0-9]+\.[0-9]+\.[0-9]+(")',
    rf'\g<1>{new_ver}\g<2>',
    cargo_lock,
    count=1,
)
cargo_lock_path.write_text(cargo_lock, encoding="utf-8")

pkg_json_path = root / "web" / "package.json"
pkg_data = json.loads(pkg_json_path.read_text(encoding="utf-8"))
pkg_data["version"] = new_ver
pkg_json_path.write_text(json.dumps(pkg_data, indent=2) + "\n", encoding="utf-8")

pkg_lock_path = root / "web" / "package-lock.json"
pkg_lock = json.loads(pkg_lock_path.read_text(encoding="utf-8"))
pkg_lock["version"] = new_ver
if "packages" in pkg_lock and "" in pkg_lock["packages"]:
    pkg_lock["packages"][""]["version"] = new_ver
pkg_lock_path.write_text(json.dumps(pkg_lock, indent=2) + "\n", encoding="utf-8")

modal_ts_path = root / "web" / "src" / "app" / "presentation" / "components" / "update-modal" / "update-modal.component.ts"
if modal_ts_path.exists():
    modal_ts = modal_ts_path.read_text(encoding="utf-8")
    modal_ts = re.sub(
        r"(current_version \|\| ')[0-9]+\.[0-9]+\.[0-9]+(')",
        rf"\g<1>{new_ver}\g<2>",
        modal_ts,
    )
    modal_ts_path.write_text(modal_ts, encoding="utf-8")

parser_ts_path = root / "web" / "src" / "app" / "infrastructure" / "parsers" / "session-snapshot.parser.ts"
if parser_ts_path.exists():
    parser_ts = parser_ts_path.read_text(encoding="utf-8")
    parser_ts = re.sub(
        r"(asString\(value\['current_version'\],\s*')[0-9]+\.[0-9]+\.[0-9]+('\))",
        rf"\g<1>{new_ver}\g<2>",
        parser_ts,
    )
    parser_ts_path.write_text(parser_ts, encoding="utf-8")

readme_path = root / "README.md"
readme = readme_path.read_text(encoding="utf-8")
readme = re.sub(r'ultradesk-rust-v[0-9]+\.[0-9]+\.[0-9]+', f'ultradesk-rust-v{new_ver}', readme)
readme = re.sub(r'ultradesk-rust_[0-9]+\.[0-9]+\.[0-9]+-1', f'ultradesk-rust_{new_ver}-1', readme)
readme = re.sub(r'ultradesk-rust-[0-9]+\.[0-9]+\.[0-9]+-1', f'ultradesk-rust-{new_ver}-1', readme)
readme = re.sub(
    r'(package-macos\.sh "\$\{TARGET\}" "\$\{ARCH\}" "v)[0-9]+\.[0-9]+\.[0-9]+(")',
    rf'\g<1>{new_ver}\g<2>',
    readme,
)
readme_path.write_text(readme, encoding="utf-8")

notes_path = root / "RELEASE_NOTES.md"
notes = notes_path.read_text(encoding="utf-8")
notes = re.sub(
    r'^## UltraDesk \(Rust Edition\) v[0-9]+\.[0-9]+\.[0-9]+',
    f'## UltraDesk (Rust Edition) v{new_ver}',
    notes,
    count=1,
    flags=re.M,
)
notes = re.sub(
    r'^Release \*\*v[0-9]+\.[0-9]+\.[0-9]+\*\* of \*\*UltraDesk \(Rust Edition\)\*\* \(`ultradesk-rust`\)[ \S]*$',
    f'Release **v{new_ver}** of **UltraDesk (Rust Edition)** (`ultradesk-rust`) — {summary}.',
    notes,
    count=1,
    flags=re.M,
)
notes_path.write_text(notes, encoding="utf-8")

today_iso = datetime.date.today().strftime("%Y-%m-%d")
metainfo_path = root / "io.github.ultradesk.UltraDesk.metainfo.xml"
metainfo = metainfo_path.read_text(encoding="utf-8")
release_tag = f'<release version="{new_ver}" date="{today_iso}" />'
if f'version="{new_ver}"' in metainfo:
    metainfo = re.sub(
        rf'<release version="{re.escape(new_ver)}" date="[^"]+"\s*/>',
        release_tag,
        metainfo,
        count=1,
    )
else:
    metainfo = metainfo.replace(
        "<releases>\n",
        f"<releases>\n    {release_tag}\n",
        1,
    )
metainfo_path.write_text(metainfo, encoding="utf-8")

nix_path = root / "nix" / "packages" / "ultradesk-rust" / "default.nix"
nix_text = nix_path.read_text(encoding="utf-8")
nix_text = re.sub(
    r'(version\s*=\s*")[0-9]+\.[0-9]+\.[0-9]+(";)',
    rf'\g<1>{new_ver}\g<2>',
    nix_text,
    count=1,
)
nix_path.write_text(nix_text, encoding="utf-8")

pkgbuild_path = root / "packaging" / "arch" / "PKGBUILD"
pkgbuild = pkgbuild_path.read_text(encoding="utf-8")
pkgbuild = re.sub(
    r'^pkgver=[0-9]+\.[0-9]+\.[0-9]+',
    f'pkgver={new_ver}',
    pkgbuild,
    count=1,
    flags=re.M,
)
pkgbuild_path.write_text(pkgbuild, encoding="utf-8")

deb_date = datetime.datetime.now(datetime.timezone(datetime.timedelta(hours=7))).strftime(
    "%a, %d %b %Y %H:%M:%S %z"
)
deb_changelog_path = root / "packaging" / "debian" / "changelog"
deb_changelog = deb_changelog_path.read_text(encoding="utf-8")
deb_changelog = re.sub(
    r'^ultradesk-rust \([0-9]+\.[0-9]+\.[0-9]+-1\) unstable; urgency=medium',
    f'ultradesk-rust ({new_ver}-1) unstable; urgency=medium',
    deb_changelog,
    count=1,
    flags=re.M,
)
bullet_line = f"  * {summary}"
if f"Release UltraDesk (Rust Edition) {new_ver}" not in deb_changelog and bullet_line not in deb_changelog:
    deb_changelog = re.sub(
        r'^(ultradesk-rust \([^\n]+\n\n)',
        rf'\g<1>{bullet_line}\n',
        deb_changelog,
        count=1,
    )
deb_changelog = re.sub(
    r'^( -- Hieu Doan <dtrunghieu96@gmail\.com>  ).+$',
    rf'\g<1>{deb_date}',
    deb_changelog,
    count=1,
    flags=re.M,
)
deb_changelog_path.write_text(deb_changelog, encoding="utf-8")

plist_path = root / "packaging" / "macos" / "Info.plist"
plist = plist_path.read_text(encoding="utf-8")
plist = re.sub(
    r'(<key>CFBundleShortVersionString</key>\s*<string>)[0-9]+\.[0-9]+\.[0-9]+(</string>)',
    rf'\g<1>{new_ver}\g<2>',
    plist,
    count=1,
)
plist = re.sub(
    r'(<key>CFBundleVersion</key>\s*<string>)[0-9]+\.[0-9]+\.[0-9]+(</string>)',
    rf'\g<1>{new_ver}\g<2>',
    plist,
    count=1,
)
plist_path.write_text(plist, encoding="utf-8")

macos_sh_path = root / "packaging" / "macos" / "package-macos.sh"
macos_sh = macos_sh_path.read_text(encoding="utf-8")
macos_sh = re.sub(
    r'RAW_VERSION="\$\{3:-v[0-9]+\.[0-9]+\.[0-9]+\}"',
    f'RAW_VERSION="${{3:-v{new_ver}}}"',
    macos_sh,
    count=1,
)
macos_sh = re.sub(
    r'sed "s\|<string>[0-9]+\.[0-9]+\.[0-9]+</string>\|<string>\$\{VERSION\}</string>\|g"',
    f'sed "s|<string>{new_ver}</string>|<string>${{VERSION}}</string>|g"',
    macos_sh,
    count=1,
)
macos_sh_path.write_text(macos_sh, encoding="utf-8")

rpm_date = datetime.date.today().strftime("%a %b %d %Y")
rpm_bullet = f"- {summary}"
for spec_rel in [
    pathlib.Path("packaging/rpm/fedora/ultradesk-rust.spec"),
    pathlib.Path("packaging/rpm/opensuse/ultradesk-rust.spec"),
]:
    spec_path = root / spec_rel
    spec = spec_path.read_text(encoding="utf-8")
    spec = re.sub(
        r'^(Version:\s+)[0-9]+\.[0-9]+\.[0-9]+',
        rf'\g<1>{new_ver}',
        spec,
        count=1,
        flags=re.M,
    )
    spec = re.sub(
        r'^(\* [A-Za-z]{3} [A-Za-z]{3} [0-9]{2} [0-9]{4} Hieu Doan <dtrunghieu96@gmail\.com> - )[0-9]+\.[0-9]+\.[0-9]+-1',
        f'* {rpm_date} Hieu Doan <dtrunghieu96@gmail.com> - {new_ver}-1',
        spec,
        count=1,
        flags=re.M,
    )
    if f"Release UltraDesk (Rust Edition) {new_ver}" not in spec and rpm_bullet not in spec:
        spec = re.sub(
            r'^(\* [^\n]+ - ' + re.escape(new_ver) + r'-1\n)',
            rf'\g<1>{rpm_bullet}\n',
            spec,
            count=1,
            flags=re.M,
        )
    spec_path.write_text(spec, encoding="utf-8")

checks = {
    "Cargo.toml": re.search(rf'^version = "{re.escape(new_ver)}"', cargo_toml_path.read_text(), re.M),
    "Cargo.lock": re.search(rf'name = "ultradesk-rust"\nversion = "{re.escape(new_ver)}"', cargo_lock_path.read_text()),
    "web/package.json": json.loads(pkg_json_path.read_text())["version"] == new_ver,
    "web/package-lock.json": json.loads(pkg_lock_path.read_text())["version"] == new_ver,
    "README.md": f"ultradesk-rust-v{new_ver}" in readme_path.read_text()
    and f'package-macos.sh "${{TARGET}}" "${{ARCH}}" "v{new_ver}"' in readme_path.read_text(),
    "RELEASE_NOTES.md": f"## UltraDesk (Rust Edition) v{new_ver}" in notes_path.read_text(),
    "io.github.ultradesk.UltraDesk.metainfo.xml": f'version="{new_ver}"' in metainfo_path.read_text(),
    "nix/packages/ultradesk-rust/default.nix": f'version = "{new_ver}";' in nix_path.read_text(),
    "packaging/arch/PKGBUILD": f"pkgver={new_ver}" in pkgbuild_path.read_text(),
    "packaging/debian/changelog": f"ultradesk-rust ({new_ver}-1)" in deb_changelog_path.read_text(),
    "packaging/macos/Info.plist": f"<string>{new_ver}</string>" in plist_path.read_text(),
    "packaging/macos/package-macos.sh": f"v{new_ver}" in macos_sh_path.read_text(),
    "packaging/rpm/fedora/ultradesk-rust.spec": f"Version:        {new_ver}" in (root / "packaging/rpm/fedora/ultradesk-rust.spec").read_text(),
    "packaging/rpm/opensuse/ultradesk-rust.spec": f"Version:        {new_ver}" in (root / "packaging/rpm/opensuse/ultradesk-rust.spec").read_text(),
}
failed = [k for k, ok in checks.items() if not ok]
if failed:
    raise SystemExit(f"Version verification failed for: {', '.join(failed)}")

print(f"Updated 14 files from {old_ver} -> {new_ver}")
PY
