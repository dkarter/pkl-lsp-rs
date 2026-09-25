# Feature parity and outside-in test plan

Reference: apple/pkl-lsp commit `47d5d5531b96072920d06d05c238f8898ba3e748`,
`README.adoc`, `PklLspServer.kt`, `PklTextDocumentService.kt`, and
`docs/modules/ROOT/pages/integration.adoc`. Legend: **partial** = only the
explicitly described behavior works, **missing** = not advertised or implemented.
Every feature must be proven with a process-level stdio test, not just a unit
test, before promotion to supported. The current tests run in `mise run test`.

| Upstream surface                                                                                     | Status                                 | Evidence / next outside-in test                                                                                                                                                 |
| ---------------------------------------------------------------------------------------------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| initialize, shutdown, exit; full text sync                                                           | partial                                | `protocol_initialization_and_shutdown`, `document_lifecycle_and_local_completion`, `requests_after_shutdown_are_rejected`, `exit_without_shutdown_fails`; add versioned changes |
| diagnostics (syntax, imports, modifiers, unused locals, etc.)                                        | missing                                | open/change → `publishDiagnostics`, valid/invalid fixtures                                                                                                                      |
| hover incl documentation                                                                             | missing                                | `textDocument/hover` over local and imported symbols                                                                                                                            |
| go to definition                                                                                     | missing                                | `textDocument/definition` across local/project/package files                                                                                                                    |
| completion incl local, qualified, module URI, string literal types, method implementation, doc links | partial: local property names only     | `document_lifecycle_and_local_completion`, `completion_uses_utf16_offsets_after_non_bmp_characters`; add grammar-context protocol fixtures                                      |
| remote package/amends schemas (e.g. public hk Config.pkl)                                            | missing                                | isolated public hk fixture → `textDocument/completion` returns schema fields; then Neovim + Blink E2E with no Java on PATH                                                      |
| completion item resolve                                                                              | missing                                | completion → `completionItem/resolve`                                                                                                                                           |
| project sync / `pkl/syncProjects`                                                                    | missing                                | fixture project → custom request → resolved dependency completion                                                                                                               |
| package download / `pkl/downloadPackage`                                                             | missing                                | local HTTP package fixture → request → verified package cache; invalid request currently returns -32601                                                                         |
| formatting                                                                                           | missing                                | `textDocument/formatting` → minimal text edits                                                                                                                                  |
| type checking                                                                                        | missing                                | module amends/type mismatch → diagnostics                                                                                                                                       |
| quick fixes / code actions                                                                           | missing                                | diagnostic range → `textDocument/codeAction` workspace edit                                                                                                                     |
| semantic tokens (upstream capability beyond README checklist)                                        | missing                                | `textDocument/semanticTokens/full` → delta-encoded tokens                                                                                                                       |
| `pkl/fileContents`                                                                                   | partial: open in-memory documents only | `document_lifecycle_and_local_completion`; add file/package URI resolution                                                                                                      |
| workspace folders and configuration                                                                  | missing                                | initialized workspace → sync/configuration updates                                                                                                                              |
| rename, references, code lens                                                                        | not offered upstream                   | out of parity scope                                                                                                                                                             |

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

The current prefix matcher is intentionally temporary; it does not understand
Pkl syntax, external packages, or the type system. Cursor prefixes use UTF-16
offsets, but the server has not been tested for every Unicode edge case.
No parity claim should be inferred from release workflow configuration.
