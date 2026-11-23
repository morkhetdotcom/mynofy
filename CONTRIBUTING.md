# Contributing to mynofy

Thank you for helping improve mynofy. Bug reports, documentation improvements,
focused fixes, and SDK interoperability work are welcome through public issues
and pull requests.

## Where contributions go

Open pull requests against `main` in
[morkhetdotcom/mynofy](https://github.com/morkhetdotcom/mynofy/pulls).
The public repository contains released source snapshots. Maintainers integrate
accepted changes into the private development repository, run the checks there,
and include them in a subsequent public release. A public PR may therefore be
closed with an integration/release reference rather than merged directly into the
public branch. Contributor attribution is retained when integrating the change.

Public PRs do not run workflows or gain access to private runners, credentials,
configuration, or unreleased history. All CI and release jobs in the development
repository run exclusively on self-hosted runners. Maintainers review external
changes before running them on that infrastructure.

## Before you start

- For a bug, include the version/commit, OS/architecture, relevant command, expected
  result, and a minimal reproduction with sanitized configuration.
- For substantial features or protocol changes, open an issue first to agree on
  scope and compatibility. Small, focused documentation fixes can go straight to PR.
- Keep dependencies few and well maintained. Explain why an added dependency is
  necessary. Do not introduce a new crypto provider or wire format casually.
- Keep resource use bounded. Avoid allocations or blocking I/O in the message hot
  path without measurements and a clear reason.

## Build and validate

Use Rust 1.88+ and the repository's locked dependencies. Install a C compiler and
CMake; SDK-specific checks also need their language runtime, OpenSSL CLI, and jq
where documented.

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

For an SDK change, run the corresponding `scripts/test-<language>-sdk.sh` as an
unprivileged user. Python tests can use `MYNOFY_PYTHON` to select an isolated
interpreter. Include useful regression coverage for changed behavior, especially
failure, disconnect, permissions, and resource cleanup paths. Do not add flaky
latency/throughput thresholds to functional tests.

Generated bindings and the catalog must match their canonical sources under
`protocol/`. Do not change generated IDs by hand. Include the matching regenerated
files when changing the schema. Explain compatibility consequences in the PR.

## What to put in a pull request

Describe the concrete problem, resulting behavior, validation performed, and any
remaining limitations. Link the issue if there is one. Update user-facing English
documentation and examples when behavior changes. Keep unrelated formatting,
refactors, and new features out of a focused fix.

Never commit credentials, generated `.local/` state, security journals, content
snapshots, production configurations, or build caches. Use synthetic test data.
The release export is a file-by-file allowlist; maintainers explicitly review any
new public file before adding it. Allowed paths still require content review.

## Review and release

Maintainers may request changes, reproduce the report, and run Linux/macOS and
cross-language checks. Acceptance does not imply an immediate release. The
private release pipeline publishes a reviewed product snapshot without private
history or CI configuration.

Contributions are accepted under the project's [MIT License](LICENSE). By
submitting a contribution, you agree to license your contribution under MIT and
confirm that you have the right to submit it. No separate CLA is required.
Keep applicable copyright and third-party license notices intact.

For security-sensitive reports, use [SECURITY.md](SECURITY.md). Communicate
respectfully, provide reproducible evidence, and keep review focused on the work.
