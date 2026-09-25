# Feature parity and outside-in test plan

Reference: apple/pkl-lsp commit `47d5d5531b96072920d06d05c238f8898ba3e748`,
`README.adoc`, `PklLspServer.kt`, `PklTextDocumentService.kt`, and
`docs/modules/ROOT/pages/integration.adoc`. Legend: **partial** = only the
explicitly described behavior works, **missing** = not advertised or implemented.
Every feature must be proven with a process-level stdio test, not just a unit
test, before promotion to supported. The current tests run in `mise run test`.

Observed on 2026-09-25: 22 offline stdio tests pass; both ignored public hk
network stdio tests pass when run explicitly. In Herdr tab `Neovim E2E`
(`w2Z:t2`), `mise run test-e2e` passed with the real Rust server, isolated
Neovim, Blink menu visible, and `min_hk_version` offered from the public hk
package while `PATH=/nonexistent`. This is evidence for that one package schema,
not for all packages or full upstream parity.

| Upstream surface                                                                                     | Status                                                    | Evidence / next outside-in test                                                                                                                                                                                                                                 |
| ---------------------------------------------------------------------------------------------------- | --------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| initialize, shutdown, exit; full text sync                                                           | partial                                                   | `protocol_initialization_and_shutdown`, `document_lifecycle_and_local_completion`, `requests_after_shutdown_are_rejected`, `exit_without_shutdown_fails`; add versioned changes                                                                                 |
| diagnostics (syntax, imports, modifiers, unused locals, etc.)                                        | partial: syntax, primitive types, explicit unused imports | `syntax_diagnostics_are_published_and_cleared_on_change`, `typed_literal_mismatch_diagnostic_clears_when_fixed`, `changing_open_base_republishes_amending_child_diagnostics`, `unused_import_produces_diagnostic_and_removal_quick_fix`; most analyzers missing |
| hover incl documentation                                                                             | partial: local/amends property type                       | `hover_and_definition_resolve_local_typed_property`, `hover_and_definition_resolve_local_amends_schema`; property docs supported, imports missing                                                                                                               |
| go to definition                                                                                     | partial: local/amends property                            | `hover_and_definition_resolve_local_typed_property`, `hover_and_definition_resolve_local_amends_schema`; package URI navigation and imports missing                                                                                                             |
| completion incl local, qualified, module URI, string literal types, method implementation, doc links | partial: local, amends and relative file URIs             | `document_lifecycle_and_local_completion`, `local_amends_schema_completion_at_root_and_in_nested_object`, `module_uri_completion_lists_neighboring_pkl_files`; most contexts missing                                                                            |
| remote package/amends schemas (e.g. public hk Config.pkl)                                            | partial: direct GitHub ZIP amends                         | `public_hk_package_schema_completes_from_release` (network, ignored by default), `mise run test-e2e` (real Neovim + Blink in Herdr); SHA-256 verified, inheritance absent                                                                                       |
| completion item resolve                                                                              | partial: property types and docs                          | `completion_item_resolves_schema_documentation`; other provider kinds missing                                                                                                                                                                                   |
| project sync / `pkl/syncProjects`                                                                    | missing                                                   | fixture project → custom request → resolved dependency completion                                                                                                                                                                                               |
| package download / `pkl/downloadPackage`                                                             | partial: in-memory verified GitHub ZIP                    | `package_download_request_makes_public_module_available` (network, ignored by default); local persistence and general registries missing                                                                                                                        |
| formatting                                                                                           | partial: simple top-level assignments                     | `formatting_normalizes_property_assignment_without_changing_string_contents`; full Pkl grammar and formatter options missing                                                                                                                                    |
| type checking                                                                                        | partial: direct primitive literals                        | `typed_literal_mismatch_diagnostic_clears_when_fixed`, `amended_property_literal_type_is_checked`; expressions, unions, collections and inherited classes missing                                                                                               |
| quick fixes / code actions                                                                           | partial: unused explicit imports                          | `unused_import_produces_diagnostic_and_removal_quick_fix`, `code_action_only_removes_requested_unused_import`; other upstream actions missing                                                                                                                   |
| semantic tokens (upstream capability beyond README checklist)                                        | partial: simple doc member links                          | `semantic_tokens_highlight_documentation_member_links`, `semantic_tokens_include_indented_doc_comment_column`; complex docs not proven                                                                                                                          |
| `pkl/fileContents`                                                                                   | partial: open docs and downloaded ZIPs                    | `document_lifecycle_and_local_completion`, `package_download_request_makes_public_module_available`; uncached file and other URI schemes missing                                                                                                                |
| workspace folders and configuration                                                                  | missing                                                   | initialized workspace → sync/configuration updates                                                                                                                                                                                                              |
| rename, references, code lens                                                                        | not offered upstream                                      | out of parity scope                                                                                                                                                                                                                                             |

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
