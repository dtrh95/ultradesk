# Code style rules

This document is the single source of truth for the code rules of UltraDesk (`ultradesk-rust`). On conflict, this document wins.

## 1. Format & lint (mandatory)

- **Rust**: Format with `cargo fmt --all` and lint with `cargo clippy --all-targets`, keeping **0 warnings in repository code**.
- **Web UI (`web/`)**: Build cleanly with `npm run build` inside `web/` and sync the bundled output to `viewer.html`.
- Any `#[allow(...)]` attribute must be scoped as narrowly as possible and accompanied by a clear reason.

## 2. Language standard

- **Rust 2021 edition** (`edition = "2021"` in `Cargo.toml`). Never downgrade.
- Prefer standard library primitives (`std::sync`, `std::net`, `std::path`, `std::io`) and existing workspace crates before adding new dependencies.

## 3. Ownership & resource management

- Every operating system handle, socket, child process, or display resource (`Display*`, `/dev/uinput`, temporary files) must have deterministic cleanup via RAII (`Drop` implementations or scoped guards).
- Never leak file descriptors, shared memory segments, or background threads across reconnects or session teardowns.
- Prefer borrowing (`&str`, `&[u8]`) for read-only parameters instead of cloning owned allocations (`String`, `Vec<u8>`) when ownership is not required.

## 4. Platform boundaries & conditional compilation

- Isolate OS-specific APIs behind explicit `#[cfg(target_os = "linux")]` and `#[cfg(target_os = "macos")]` boundaries in `src/host.rs`, `src/gui.rs`, `src/clipboard.rs`, and `src/profile.rs`.
- Keep protocol framing (`src/protocol.rs`, `src/e2e.rs`), screen decoding (`src/screen.rs`), audio codecs (`src/audio.rs`), and file transfers (`src/file_transfer.rs`) platform-independent.

## 5. Security & boundary validation

- Validate all untrusted network frames, file transfer paths, and HTTP portal inputs at the boundary before acting on them.
- Confine incoming file transfers strictly to the designated download directory and reject path traversal components (`..`, absolute paths, or symlink escapes).
- Enforce brute-force lockout (`LoginThrottle`) and constant-time or cryptographic verification on authentication paths.

## 6. Module structure

- `src/main.rs` owns CLI argument parsing, startup wiring, and process lifecycle.
- `src/lib.rs` exports reusable modules for integration tests and benchmarks.
- Each domain lives in its own focused module (`gui`, `host`, `session`, `server`, `screen`, `protocol`, `e2e`, `input`, `cursor`, `audio`, `clipboard`, `file_transfer`, `direct`, `profile`).

## 7. Naming

- Types, traits, and enum variants: `PascalCase` (`HostSession`, `MonitorSelection`, `DesktopBackend`).
- Functions, methods, variables, and modules: `snake_case` (`ensure_macos_host_permissions`, `frame_version`).
- Constants and statics: `UPPER_SNAKE_CASE` (`DEFAULT_APP_ICON_PNG`, `PACKET_CURSOR_IMAGE`).

## 8. Logging & errors

- Return structured `Result<T, String>` or typed errors across fallible boundaries instead of panicking with `.unwrap()` on external inputs.
- Write error messages in English, stating the failed operation and the relevant context.
- Avoid noisy logs inside hot frame-capture or audio-decoding loops.

## 9. Comments & documentation

- Write all code comments and documentation in English.
- Keep comments only where they explain a non-obvious *why* or protocol invariant that the code cannot express on its own.
- Leave no commented-out code, temporary debug prints, or orphan `TODO`/`FIXME` notes before opening a pull request.

## 10. Tests

- Every new behavior or bug fix requires a unit or integration test (`cargo test --release`).
- Tests must be deterministic and run cleanly in headless CI environments on both Linux and macOS without requiring a live physical display.

## 11. PR scope (one PR, one problem)

- **Each PR solves exactly one problem.** If you have multiple unrelated changes, split them into separate branches and pull requests.
- Do not mix unrelated refactors, formatting sweeps, renames, or documentation rewrites into a bug-fix or feature PR.
- Every PR must build with 0 warnings, pass `cargo test --release`, and use a concise commit message following `type(scope): short description`.
