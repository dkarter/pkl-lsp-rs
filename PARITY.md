# Feature parity and outside-in test plan

Reference: apple/pkl-lsp commit `47d5d5531b96072920d06d05c238f8898ba3e748`,
`README.adoc`, `PklLspServer.kt`, `PklTextDocumentService.kt`, and
`docs/modules/ROOT/pages/integration.adoc`. Legend: **partial** = only the
explicitly described behavior works, **missing** = not advertised or implemented.
Every feature must be proven with a process-level stdio test, not just a unit
test, before promotion to supported. The current tests run in `mise run test`.

Observed on 2026-09-25: 26 offline stdio tests pass; both ignored public hk
network stdio tests pass when run explicitly. In Herdr tab `Neovim E2E`
(`w2Z:t2`), `mise run test-e2e` passed with the real Rust server, isolated
Neovim, Blink menu visible, and `min_hk_version` offered from the public hk
package while `PATH=/nonexistent`. This is evidence for that one package schema,
not for all packages or full upstream parity.

| Upstream surface                                                                                     | Status                                                    | Evidence / next outside-in test                                                                                                                                                                                                                                                                                                                            |
| ---------------------------------------------------------------------------------------------------- | --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| initialize, shutdown, exit; full text sync                                                           | partial                                                   | `protocol_initialization_and_shutdown`, `document_lifecycle_and_local_completion`, `stale_or_ranged_full_sync_changes_do_not_replace_newer_document`; malformed JSON-RPC and broader lifecycle cases missing                                                                                                                                               |
| diagnostics (syntax, imports, modifiers, unused locals, etc.)                                        | partial: syntax, primitive types, explicit unused imports | `syntax_diagnostics_are_published_and_cleared_on_change`, `typed_literal_mismatch_diagnostic_clears_when_fixed`, `changing_open_base_republishes_amending_child_diagnostics`, `unused_import_produces_diagnostic_and_removal_quick_fix`; most analyzers missing                                                                                            |
| hover incl documentation                                                                             | partial: local/amends/qualified local import property     | `hover_and_definition_resolve_local_typed_property`, `qualified_import_property_hover_and_definition_use_open_module`; other expressions and module forms missing                                                                                                                                                                                          |
| go to definition                                                                                     | partial: local/amends/qualified local import property     | `hover_and_definition_resolve_local_amends_schema`, `qualified_import_property_hover_and_definition_use_open_module`; package URI navigation and other imports missing                                                                                                                                                                                     |
| completion incl local, qualified, module URI, string literal types, method implementation, doc links | partial: local, amends, qualified imports and file URIs   | `qualified_import_completion_uses_open_module_not_local_properties`, `qualified_import_completion_reads_local_fixture_from_disk`, `module_uri_completion_lists_neighboring_pkl_files`; empty member prefixes and most contexts missing                                                                                                                     |
| remote package/amends schemas (e.g. public hk Config.pkl)                                            | partial: direct GitHub ZIP amends                         | `public_hk_package_schema_completes_from_release` (network, ignored by default), `mise run test-e2e` (real Neovim + Blink in Herdr); SHA-256 verified, inheritance absent                                                                                                                                                                                  |
| completion item resolve                                                                              | partial: property types and docs                          | `completion_item_resolves_schema_documentation`; other provider kinds missing                                                                                                                                                                                                                                                                              |
| project sync / `pkl/syncProjects`                                                                    | partial: disk-only literal local imports                  | `sync_projects_resolves_literal_local_dependency_after_explicit_sync`; workspace-root subset, no CLI, lockfiles, remote/transitive resolution, discovery, auto-sync, or persistence; strict syntax and size limits in README                                                                                                                               |
| package download / `pkl/downloadPackage`                                                             | partial: in-memory verified GitHub ZIP                    | `package_download_request_makes_public_module_available` (network, ignored by default); local persistence and general registries missing                                                                                                                                                                                                                   |
| formatting                                                                                           | partial: nested declaration spacing and body indentation  | `formatting_nested_classes_objects_entries_and_comments_is_idempotent`, `formatting_preserves_raw_escaped_interpolated_strings_and_comment_tokens`, `formatting_honors_indentation_options_and_crlf`, `formatting_uses_utf16_edit_ranges`, `formatting_declines_multiline_strings_continuations_and_invalid_input`; full upstream formatter parity missing |
| type checking                                                                                        | partial: direct primitive literals                        | `typed_literal_mismatch_diagnostic_clears_when_fixed`, `amended_property_literal_type_is_checked`; expressions, unions, collections and inherited classes missing                                                                                                                                                                                          |
| quick fixes / code actions                                                                           | partial: unused explicit imports                          | `unused_import_produces_diagnostic_and_removal_quick_fix`, `code_action_only_removes_requested_unused_import`; other upstream actions missing                                                                                                                                                                                                              |
| semantic tokens (upstream capability beyond README checklist)                                        | partial: simple doc member links                          | `semantic_tokens_highlight_documentation_member_links`, `semantic_tokens_include_indented_doc_comment_column`; complex docs not proven                                                                                                                                                                                                                     |
| `pkl/fileContents`                                                                                   | partial: open docs and downloaded ZIPs                    | `document_lifecycle_and_local_completion`, `package_download_request_makes_public_module_available`; uncached file and other URI schemes missing                                                                                                                                                                                                           |
| workspace folders and configuration                                                                  | partial: initial local project roots                      | `sync_projects_rejects_unsupported_declarations_and_non_file_workspaces`, `project_resync_reloads_disk_and_invalidates_failed_or_removed_mappings` (rootUri fallback); runtime folder changes and configuration updates missing                                                                                                                            |
| rename, references, code lens                                                                        | not offered upstream                                      | out of parity scope                                                                                                                                                                                                                                                                                                                                        |

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
package retrieval now uses bounded background workers; only the requesting
package-dependent response waits for verification, not the protocol loop.
The offline process tests `slow_schema_fetch_does_not_block_documents_requests_or_shutdown`
and `unavailable_download_does_not_block_documents_requests_or_shutdown`
hold a local fixture's metadata response while proving document change/close,
unrelated requests, shutdown and exit remain responsive.
`verified_download_is_deduplicated_and_cached_for_completion_and_contents`,
`checksum_failure_is_not_cached_as_verified_or_retried_immediately`, and
`cancelled_completion_does_not_block_or_publish_a_late_response` cover verified
cache reuse, negative caching and request cancellation. The two public hk
stdio tests also pass with deferred responses. The coordinator must rerun the
Neovim/Blink E2E after merge; the earlier E2E evidence above is not evidence for
this change. SHA-256 is checked
against public metadata but is not a complete trust policy.
No parity claim should be inferred from release workflow configuration.

## Formatting scope

The independent formatter normalizes same-line assignment spacing for class
and object properties (including typed properties) and object entries, spacing
before property amendment bodies, class/object body indentation, and a missing
final newline. It honors LSP `insertSpaces` and `tabSize` (1–16), preserves
existing line endings, and uses UTF-16 edit ranges. Comments and single-line
strings (including raw, escaped, and interpolated forms) retain their exact
token contents; block/doc comment interiors are not reindented. Edits are
reparsed and rejected unless syntax structure and token contents are unchanged.

Documents with multiline strings, unsupported multiline expression/continuation
layouts (including multiline methods and generators), syntax errors, invalid options, or excessive size/depth receive no
edits, including no final-newline edit. This is not a full pretty-printer:
operator/type spacing, line wrapping, blank-line policy, comment reflow, and
upstream configuration/grammar-version behavior remain missing. Upstream at
the reference commit delegates to `org.pkl.formatter.Formatter` in
`features/FormattingFeature.kt`; these tests establish a bounded independent
subset, not output equivalence with that formatter.

Resource limits are 1 MiB input/output, 16 KiB per input line, 128 syntax-tree
levels, and 16,384 visited nodes/edits. The process tests
`formatting_many_sibling_bodies_preserves_inline_expression_layout` and
`formatting_declines_excessive_input_work_and_output_expansion` cover repeated
bodies and safe refusal at these bounds. Formatting remains synchronous;
there is no parser deadline or cancellation support.

Observed on 2026-10-02 after rebasing the asynchronous package change onto
the nested formatter and local project sync: 54 offline stdio
tests pass, alongside size-limit and unsupported-host unit tests. Both ignored
public hk network stdio tests pass when run explicitly. Queue/worker limit,
EOF cancellation and current-document replay tests supplement the cases above.
Package inheritance, persistence, general registries and a complete trust policy
remain missing; local file reads and parsing are still synchronous.
