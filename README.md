# pkl-lsp-rs

An **experimental**, independent Rust implementation of the Pkl Language Server.
**Do not use it as a drop-in replacement yet.** Today it implements LSP stdio framing, full-document
open/change/close, syntax and limited type diagnostics, local property hover/definition,
basic local and amends-schema completion (including public GitHub release
packages), limited formatting, an unused-import quick fix, verified public
package downloads, and `pkl/fileContents` for open documents and downloaded
package members. The exact limits are tracked in
[PARITY.md](PARITY.md).

## Try it

```sh
mise install
mise run test
mise run lint
mise run fmt-check
cargo run --quiet
mise run test-e2e # optional: isolated Neovim + Blink; downloads public hk package
```

Point an LSP client at `target/debug/pkl-lsp-rs` over stdio. The binary reads
LSP `Content-Length` frames on stdin and writes frames on stdout. It does not
need a Pkl installation for the currently implemented behavior. `test-e2e`
requires Neovim, a local Blink checkout (`BLINK_PLUGIN` can override its path),
and network access to a **public** hk release. It sets `PATH=/nonexistent` for
Neovim and the server, and loads no user configuration or private dotfiles.
Completion currently supports local top-level properties and direct properties
from a local `amends` file or a public GitHub-hosted package ZIP; it does not
resolve imports, dependencies, inherited class hierarchies, or arbitrary Pkl
expressions. GitHub package ZIPs are size-limited and SHA-256 checked against
the package's public metadata. This is not a full Pkl package trust or
dependency-resolution implementation; do not open untrusted packages yet.
Formatting currently handles only spacing around simple top-level assignments
and the final newline. Type checking handles only direct primitive literals
with explicit or directly amended property types. Downloads remain in memory
for the lifetime of the server process.

## Release preparation (not activated)

The release-please configuration consumes **atomic conventional commits**.
After CI succeeds on `main`, the workflow proposes a draft release and builds
Linux x86_64/arm64, macOS arm64, and Windows x86_64 native archives with mbx
build caching and GitHub build attestations. This has **not** been verified in CI.
To enable it, the owner must review the remote repository and configure
`RELEASE_CLIENT_ID` and `RELEASE_PRIVATE_KEY` for
a GitHub App authorized for contents and pull requests, and review the workflow
and release policy. No tokens or settings are managed by this project.

**Immutable releases are a GitHub repository setting**, not an effect of this
workflow. The owner must explicitly enable and verify it in the repository
settings on the remote repository. Release assets are uploaded only to a
draft and the release is published after all matrix builds succeed; publishing
does not itself make a release immutable.

## Attribution and scope

The parity inventory was researched against
[`apple/pkl-lsp` at `47d5d5531b96072920d06d05c238f8898ba3e748`](https://github.com/apple/pkl-lsp/tree/47d5d5531b96072920d06d05c238f8898ba3e748),
Apache-2.0 licensed (see its `LICENSE.txt` and `NOTICE.txt`). This project
does not include upstream Kotlin sources, its native parser, or its runtime
dependencies. Code in this repository is MIT licensed; any future vendoring
of upstream sources or grammars must preserve their license and notices. The
native `apple/tree-sitter-pkl` grammar is an Apache-2.0 dependency at
`c95d8284940f5e1da2cd0d8f1ee45d7ef9ef75d1`; its license is included in
[`THIRD_PARTY_LICENSES/tree-sitter-pkl-LICENSE.txt`](THIRD_PARTY_LICENSES/tree-sitter-pkl-LICENSE.txt).
