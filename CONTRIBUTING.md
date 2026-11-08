# Contributing to UltraDesk

Thank you for your interest in contributing to the UltraDesk (`ultradesk-rust`) project! This document guides you on how to participate in project development.

## 📋 Table of Contents

- [Getting Started](#getting-started)
- [Contribution Workflow](#contribution-workflow)
- [Code of Conduct](#code-of-conduct)
- [Code Style Rules](#code-style-rules)
- [Pull Request Process](#pull-request-process)
- [Important Notes on Branching](#important-notes-on-branching)
- [Bug Reporting](#bug-reporting)
- [Feature Requests](#feature-requests)

## Getting Started

### System Requirements

- Rust toolchain (`rustc` and `cargo`, stable channel)
- Node.js >= 20 and `npm` (for building the `web/` frontend when editing UI)
- `pkg-config`, `openssl`, `zstd`
- **Linux**: `libxcb`, `gtk3`, `webkit2gtk-4.1`, `libsoup-3.0`
- **macOS**: Xcode Command Line Tools (`xcode-select --install`)
- Git

### Installation and Build

```bash
# Clone repository
git clone https://github.com/dtrh95/ultradesk.git
cd ultradesk

# Build release binary
cargo build --release

# Run test suite
cargo test --release
```

## Contribution Workflow

### 1. Fork repository

Fork this repository on GitHub and clone your fork to your machine:

```bash
git clone https://github.com/yourusername/ultradesk.git
cd ultradesk
git remote add upstream https://github.com/dtrh95/ultradesk.git
```

### 2. Create a new branch

Always create a new branch for each feature or bug fix:

```bash
git checkout main
git pull upstream main
git checkout -b feat/feature-name
# or
git checkout -b fix/bug-name
```

### 3. Make changes

- Write clean, readable code
- Follow code style rules (see below)
- Add or update tests for your changes
- Update documentation if needed

### 4. Commit changes

Use clear and descriptive commit messages following Conventional Commits:

```text
feat(host): support multi-monitor selection on macOS

- Add display bounds enumeration via CoreGraphics
- Update coordinate translation for secondary displays
```

## Code of Conduct

All members contributing to this project must adhere to the [Contributor Code of Conduct](CODE_OF_CONDUCT.md) to build an open, friendly, and healthy community.

## Code Style Rules

All rules live in [CODE_STYLE.md](CODE_STYLE.md), which is the single source of truth.

- **Rust**: `cargo fmt` + `cargo clippy --all-targets` (configuration in `Cargo.toml`).
- **Web UI (`web/`)**: `npm run build` in `web/` when modifying frontend assets.
- You are encouraged to create a `pre-commit` hook to automatically format and check code before committing by creating `.git/hooks/pre-commit` with the following content:

```bash
#!/bin/bash
set -e

RUST_FILES=$(git diff --cached --name-only --diff-filter=ACMR | grep -E '\.rs$' || true)
if [ -n "$RUST_FILES" ]; then
    cargo fmt --all
    for file in $RUST_FILES; do
        git add "$file"
    done
fi
```

Then make the hook executable: `chmod +x .git/hooks/pre-commit`

## Pull Request Process

**Each PR solves exactly one problem.** If you change several things, split them into several PRs.

### 1. Ensure clean code with `cargo fmt` and `cargo clippy`

```bash
cargo fmt --all -- --check
cargo clippy --all-targets
```

### 2. Run tests

```bash
cargo test --release
```

### 3. Rebase with the latest upstream branch

```bash
git fetch upstream
git rebase upstream/main
```

### 4. Push and create PR

```bash
git push origin feat/feature-name
```

Create a Pull Request on GitHub with:

- Clear descriptive title
- Detailed description of changes
- Link to related issue (if any)
- Screenshots for UI changes

## Important Notes on Branching

### Branch Structure and Releases

- Only the repository maintainer (`dtrh95`) is authorized to push release tags (`v*`) to trigger official builds and package publishing.
- Every pull request must pass all automated GitHub Actions checks (`cargo test --release` on Linux and macOS, plus Nix flake checks) before review and merge.
- Create a dedicated branch (`feat/*`, `fix/*`, `docs/*`, `ci/*`) for each pull request.

## Bug Reporting

When reporting a bug, please use the [Bug Report template](https://github.com/dtrh95/ultradesk/issues/new?template=bug_report.yml) and provide:

- UltraDesk version (`ultradesk-rust --version` or release package version)
- Operating system and version (e.g., Ubuntu 24.04, Arch Linux, macOS Sequoia 15.3)
- Display server or desktop environment (Wayland, X11, macOS Aqua)
- Steps to reproduce the bug
- Terminal log output or screenshot (if any)

## Feature Requests

Before proposing a new feature:

- Check if the feature already exists
- Search for similar issues
- Clearly describe the feature and use case using the [Feature Request template](https://github.com/dtrh95/ultradesk/issues/new?template=feature_request.yml)
- Explain why this feature is necessary

## License

By contributing, you agree that your code will be licensed under the same license as the project ([MIT License](LICENSE)).

---

Thank you for contributing!
