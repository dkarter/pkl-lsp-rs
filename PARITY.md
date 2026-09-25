# Feature parity and outside-in test plan

Reference: apple/pkl-lsp commit `47d5d5531b96072920d06d05c238f8898ba3e748`,
`README.adoc`, `PklLspServer.kt`, `PklTextDocumentService.kt`, and
`docs/modules/ROOT/pages/integration.adoc`. Legend: **partial** = only the
explicitly described behavior works, **missing** = not advertised or implemented.
Every feature must be proven with a process-level stdio test, not just a unit
test, before promotion to supported. The current tests run in `mise run test`.

Observed on 2026-09-25: 13 offline stdio tests pass; the ignored public hk
network stdio test passes when run explicitly. In Herdr tab `Neovim E2E`
(`w2Z:t2`), `mise run test-e2e` passed with the real Rust server, isolated
Neovim, Blink menu visible, and `min_hk_version` offered from the public hk
package while `PATH=/nonexistent`. This is evidence for that one package schema,
not for all packages or full upstream parity.

| Upstream surface                                                                                     | Status                                 | Evidence / next outside-in test                                                                                                                                                           |
| ---------------------------------------------------------------------------------------------------- | -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| initialize, shutdown, exit; full text sync                                                           | partial                                | `protocol_initialization_and_shutdown`, `document_lifecycle_and_local_completion`, `requests_after_shutdown_are_rejected`, `exit_without_shutdown_fails`; add versioned changes           |
| diagnostics (syntax, imports, modifiers, unused locals, etc.)                                        | partial: syntax errors only            | `syntax_diagnostics_are_published_and_cleared_on_change`; semantic analyzers missing                                                                                                      |
| hover incl documentation                                                                             | partial: local/amends property type    | `hover_and_definition_resolve_local_typed_property`, `hover_and_definition_resolve_local_amends_schema`; docs and imports missing                                                         |
| go to definition                                                                                     | partial: local/amends property         | `hover_and_definition_resolve_local_typed_property`, `hover_and_definition_resolve_local_amends_schema`; package URI navigation and imports missing                                       |
| completion incl local, qualified, module URI, string literal types, method implementation, doc links | partial: local and direct amends names | `document_lifecycle_and_local_completion`, `completion_uses_utf16_offsets_after_non_bmp_characters`, `local_amends_schema_completion_at_root_and_in_nested_object`; most contexts missing |
| remote package/amends schemas (e.g. public hk Config.pkl)                                            | partial: direct GitHub ZIP amends      | `public_hk_package_schema_completes_from_release` (network, ignored by default), `mise run test-e2e` (real Neovim + Blink in Herdr); SHA-256 verified, inheritance absent                 |
| completion item resolve                                                                              | missing                                | completion → `completionItem/resolve`                                                                                                                                                     |
| project sync / `pkl/syncProjects`                                                                    | missing                                | fixture project → custom request → resolved dependency completion                                                                                                                         |
| package download / `pkl/downloadPackage`                                                             | missing                                | local HTTP package fixture → request → verified package cache; invalid request currently returns -32601                                                                                   |
| formatting                                                                                           | missing                                | `textDocument/formatting` → minimal text edits                                                                                                                                            |
| type checking                                                                                        | missing                                | module amends/type mismatch → diagnostics                                                                                                                                                 |
| quick fixes / code actions                                                                           | missing                                | diagnostic range → `textDocument/codeAction` workspace edit                                                                                                                               |
| semantic tokens (upstream capability beyond README checklist)                                        | partial: simple doc member links       | `semantic_tokens_highlight_documentation_member_links`; nested links and complex docs not proven                                                                                          |
| `pkl/fileContents`                                                                                   | partial: open in-memory documents only | `document_lifecycle_and_local_completion`; add file/package URI resolution                                                                                                                |
| workspace folders and configuration                                                                  | missing                                | initialized workspace → sync/configuration updates                                                                                                                                        |
| rename, references, code lens                                                                        | not offered upstream                   | out of parity scope                                                                                                                                                                       |

## Implementation milestones

1. **Protocol foundation (started):** reliable JSON-RPC framing and document
   store, UTF-16 positions, correct incremental/full sync, graceful shutdown,
   diagnostics publication, public stdio tests first.
2. **Parser and index:** error-tolerant Pkl grammar (evaluate the public
   `apple/tree-sitter-pkl` grammar's license and distribution), symbol index,
   module imports, stdlib, local diagnostics/hover/definition/completion.
3. **Schemas and projects:** HTTP package resolution, authenticated requests
   via client configuration, checksum validation and cache, amends/type
   inference, project sync; prove public hk schema completion over stdio and
   through an isolated headless Neovim + Blink run. Never load private dotfiles.
4. **Remaining parity:** formatter, semantic tokens, quick fixes, other
   analyzers; port _behavioral fixtures_ from upstream where licenses permit
   while implementing independently in idiomatic Rust.
5. **Release readiness:** CI matrix execution on a user-approved remote,
   attestations, immutable-release repository setting verified by owner.

The current prefix matcher is intentionally temporary; the parser extracts
top-level and direct class declarations, but it does not infer types, inspect
imported modules, or traverse inherited class hierarchies. Cursor prefixes
use UTF-16 offsets, but every Unicode edge case has not been tested. Remote
package retrieval currently blocks the protocol loop; SHA-256 is checked
against public metadata but is not a complete trust policy.
No parity claim should be inferred from release workflow configuration.
