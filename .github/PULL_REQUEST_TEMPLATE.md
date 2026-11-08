# Description

<!-- Clearly and concisely describe your changes: what problem this solves and how it works. -->

## Type of change

- [ ] New feature
- [ ] Bug fix
- [ ] Performance improvement
- [ ] Documentation update
- [ ] CI/CD or packaging improvement

## Related issue

<!-- Fixes #123 (if applicable) -->

## Screenshots (for UI changes)

<!-- Paste before/after screenshots if this PR modifies the user interface -->

# Checklist

- [ ] I have read the project's [`CONTRIBUTING.md`](../CONTRIBUTING.md) and [`CODE_STYLE.md`](../CODE_STYLE.md)
- [ ] This PR solves **one** single problem (multiple unrelated changes are split into separate PRs)
- [ ] No unrelated refactors, formatting sweeps, or out-of-scope documentation changes are mixed in
- [ ] Code is formatted with `cargo fmt --all -- --check`
- [ ] Code passes `cargo clippy --all-targets` with 0 warnings
- [ ] Release build succeeds (`cargo build --release`)
- [ ] All unit and integration tests pass (`cargo test --release`)
- [ ] Branch is rebased onto the latest `main` branch
- [ ] All CI checks pass
