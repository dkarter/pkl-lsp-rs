# pkl-lsp-rs

An **experimental**, independent Rust implementation of the Pkl Language Server.
It does not execute Java, the JVM, or the upstream server. **Do not use it as a
drop-in replacement yet.** Today it implements LSP stdio framing, full-document
open/change/close, simple local top-level property completion, and in-memory
`pkl/fileContents`. Everything else is tracked in [PARITY.md](PARITY.md).

## Try it

```sh
mise install
mise run test
mise run lint
mise run fmt-check
cargo run --quiet
```

Point an LSP client at `target/debug/pkl-lsp-rs` over stdio. The binary reads
LSP `Content-Length` frames on stdin and writes frames on stdout. It does not
need a Pkl installation for the currently implemented behavior. It does **not**
yet load external modules or package schemas; in particular, it cannot provide
hk schema completion.

## Release preparation (not activated)

The release-please configuration consumes **atomic conventional commits**.
After CI succeeds on `main`, the workflow proposes a draft release and builds
Linux x86_64/arm64, macOS arm64, and Windows x86_64 native archives with mbx
build caching and GitHub build attestations. This has **not** run in CI: no
remote repository exists yet. To enable it later, the owner must create a
remote repository, configure `RELEASE_CLIENT_ID` and `RELEASE_PRIVATE_KEY` for
a GitHub App authorized for contents and pull requests, and review the workflow
and release policy. No tokens or settings are managed by this project.

**Immutable releases are a GitHub repository setting**, not an effect of this
workflow. The owner must explicitly enable and verify it in the repository
settings after creating the repository. Release assets are uploaded only to a
draft and the release is published after all matrix builds succeed; publishing
does not itself make a release immutable.

## Attribution and scope

The parity inventory was researched against
[`apple/pkl-lsp` at `47d5d5531b96072920d06d05c238f8898ba3e748`](https://github.com/apple/pkl-lsp/tree/47d5d5531b96072920d06d05c238f8898ba3e748),
Apache-2.0 licensed (see its `LICENSE.txt` and `NOTICE.txt`). This project
does not include upstream Kotlin sources, its native parser, or its runtime
dependencies. Code in this repository is MIT licensed; any future vendoring
of upstream sources or grammars must preserve their license and notices.
