use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn exchange(messages: &[Value]) -> Vec<Value> {
    exchange_all(messages)
        .into_iter()
        .filter(|response| response.get("id").is_some())
        .collect()
}

fn exchange_all(messages: &[Value]) -> Vec<Value> {
    let mut process = Command::new(env!("CARGO_BIN_EXE_pkl-lsp-rs"))
        // Run the real child process without unrelated executables on PATH.
        .env("PATH", "/nonexistent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = process.stdin.take().unwrap();
    let messages = messages.to_vec();
    let writer = std::thread::spawn(move || {
        for message in messages {
            let bytes = serde_json::to_vec(&message).unwrap();
            write!(stdin, "Content-Length: {}\r\n\r\n", bytes.len()).unwrap();
            stdin.write_all(&bytes).unwrap();
        }
    });
    let output = process.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut cursor = output.stdout.as_slice();
    let mut responses = Vec::new();
    while !cursor.is_empty() {
        let header_end = cursor
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        let headers = std::str::from_utf8(&cursor[..header_end]).unwrap();
        let length: usize = headers
            .strip_prefix("Content-Length: ")
            .unwrap()
            .parse()
            .unwrap();
        cursor = &cursor[header_end + 4..];
        responses.push(serde_json::from_slice(&cursor[..length]).unwrap());
        cursor = &cursor[length..];
    }
    responses
}

#[test]
fn syntax_diagnostics_are_published_and_cleared_on_change() {
    let messages = exchange_all(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/invalid.pkl","languageId":"pkl","version":1,"text":"thing = ("}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///tmp/invalid.pkl","version":2},"contentChanges":[{"text":"thing = 1"}]}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    let published: Vec<_> = messages
        .iter()
        .filter(|m| m["method"] == "textDocument/publishDiagnostics")
        .collect();
    assert_eq!(published.len(), 2);
    assert!(
        !published[0]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(published[1]["params"]["diagnostics"], json!([]));
    assert_eq!(published[1]["params"]["version"], 2);
}

#[test]
fn stale_or_ranged_full_sync_changes_do_not_replace_newer_document() {
    let uri = "file:///tmp/versioned.pkl";
    let messages = exchange_all(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pkl","version":4,"text":"original: String = 1"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":5},"contentChanges":[{"text":"current: Int = 1"}]}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":3},"contentChanges":[{"text":"stale: String = 1"}]}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":6},"contentChanges":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"text":"corrupt"}]}}),
        json!({"jsonrpc":"2.0","id":2,"method":"pkl/fileContents","params":{"uri":uri}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["method"] == "textDocument/publishDiagnostics")
            .count(),
        2
    );
    assert_eq!(
        messages.iter().find(|m| m["id"] == 2).unwrap()["result"],
        "current: Int = 1"
    );
}

#[test]
fn typed_literal_mismatch_diagnostic_clears_when_fixed() {
    let messages = exchange_all(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/typed.pkl","languageId":"pkl","version":1,"text":"name: String = 42"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///tmp/typed.pkl","version":2},"contentChanges":[{"text":"name: String = \"ok\""}]}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    let diagnostics: Vec<_> = messages
        .iter()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .collect();
    assert_eq!(
        diagnostics[0]["params"]["diagnostics"][0]["code"],
        "type-mismatch"
    );
    assert_eq!(
        diagnostics[0]["params"]["diagnostics"][0]["range"]["start"],
        json!({"line":0,"character":15})
    );
    assert_eq!(diagnostics[1]["params"]["diagnostics"], json!([]));
}

#[test]
fn amended_property_literal_type_is_checked() {
    let messages = exchange_all(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/typed-base.pkl","languageId":"pkl","version":1,"text":"module Base\nname: String"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/typed-child.pkl","languageId":"pkl","version":1,"text":"amends \"./typed-base.pkl\"\nname = 42"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    let diagnostics: Vec<_> = messages
        .iter()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .collect();
    assert_eq!(
        diagnostics[1]["params"]["diagnostics"][0]["code"],
        "type-mismatch"
    );
    assert_eq!(
        diagnostics[1]["params"]["diagnostics"][0]["message"],
        "Expected String, found Int"
    );
}

#[test]
fn changing_open_base_republishes_amending_child_diagnostics() {
    let messages = exchange_all(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/recheck-base.pkl","languageId":"pkl","version":1,"text":"module Base\nname: String"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/recheck-child.pkl","languageId":"pkl","version":1,"text":"amends \"./recheck-base.pkl\"\nname = 42"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///tmp/recheck-base.pkl","version":2},"contentChanges":[{"text":"module Base\nname: Int"}]}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    let child: Vec<_> = messages
        .iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == "file:///tmp/recheck-child.pkl"
        })
        .collect();
    assert_eq!(child.len(), 2);
    assert_eq!(
        child[0]["params"]["diagnostics"][0]["code"],
        "type-mismatch"
    );
    assert_eq!(child[1]["params"]["diagnostics"], json!([]));
}

#[test]
fn unused_import_produces_diagnostic_and_removal_quick_fix() {
    let messages = exchange_all(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/imports.pkl","languageId":"pkl","version":1,"text":"import \"./schema.pkl\" as Schema\nvalue = 1\n"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction","params":{"textDocument":{"uri":"file:///tmp/imports.pkl"},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":31}},"context":{"diagnostics":[{"code":"unused-import","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":31}}}]}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    let published = messages
        .iter()
        .find(|message| message["method"] == "textDocument/publishDiagnostics")
        .unwrap();
    assert_eq!(
        published["params"]["diagnostics"][0]["code"],
        "unused-import"
    );
    let action = messages.iter().find(|message| message["id"] == 2).unwrap();
    assert_eq!(action["result"][0]["kind"], "quickfix");
    assert_eq!(
        action["result"][0]["edit"]["changes"]["file:///tmp/imports.pkl"][0]["newText"],
        ""
    );
    assert_eq!(
        action["result"][0]["edit"]["changes"]["file:///tmp/imports.pkl"][0]["range"]["end"],
        json!({"line":1,"character":0})
    );
}

#[test]
fn formatting_normalizes_property_assignment_without_changing_string_contents() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/format.pkl","languageId":"pkl","version":1,"text":"answer=42\nmessage=\"a=b\""}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{"textDocument":{"uri":"file:///tmp/format.pkl"},"options":{"tabSize":2,"insertSpaces":true}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///tmp/format.pkl","version":2},"contentChanges":[{"text":"answer = 42\nmessage = \"a=b\"\n"}]}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/formatting","params":{"textDocument":{"uri":"file:///tmp/format.pkl"},"options":{"tabSize":2,"insertSpaces":true}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(
        result[0]["result"]["capabilities"]["documentFormattingProvider"],
        true
    );
    let edits = result[1]["result"].as_array().unwrap();
    assert!(edits.iter().any(|edit| edit["newText"] == " = "));
    assert!(edits.iter().any(|edit| edit["newText"] == "\n"));
    assert!(
        !edits
            .iter()
            .any(|edit| edit["newText"].as_str().unwrap().contains("a = b"))
    );
    assert_eq!(result[2]["result"], json!([]));
}

#[test]
fn hover_and_definition_resolve_local_typed_property() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/symbol.pkl","languageId":"pkl","version":1,"text":"greeting: String = \"hello\"\nresult = greeting"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///tmp/symbol.pkl"},"position":{"line":1,"character":13}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///tmp/symbol.pkl"},"position":{"line":1,"character":13}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert!(
        result[1]["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("greeting: String")
    );
    assert_eq!(result[2]["result"][0]["uri"], "file:///tmp/symbol.pkl");
    assert_eq!(
        result[2]["result"][0]["range"]["start"],
        json!({"line":0,"character":0})
    );
}

#[test]
fn qualified_import_property_hover_and_definition_use_open_module() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/import-base.pkl","languageId":"pkl","version":1,"text":"module Base\n/// Imported property.\ngreeting: String"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/import-child.pkl","languageId":"pkl","version":1,"text":"import \"./other.pkl\"\nimport \"./import-base.pkl\" as Base\nvalue = Base.greeting"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///tmp/import-child.pkl"},"position":{"line":2,"character":16}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///tmp/import-child.pkl"},"position":{"line":2,"character":16}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert!(
        result[1]["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Imported property.")
    );
    assert_eq!(result[2]["result"][0]["uri"], "file:///tmp/import-base.pkl");
    assert_eq!(
        result[2]["result"][0]["range"]["start"],
        json!({"line":2,"character":0})
    );
}

#[test]
fn hover_and_definition_resolve_local_amends_schema() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/base.pkl","languageId":"pkl","version":1,"text":"module Base\nfeatureFlag: Boolean"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/derived.pkl","languageId":"pkl","version":1,"text":"amends \"./base.pkl\"\nfeatureFlag = true"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///tmp/derived.pkl"},"position":{"line":1,"character":6}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///tmp/derived.pkl"},"position":{"line":1,"character":6}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert!(
        result[1]["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("featureFlag: Boolean")
    );
    assert_eq!(result[2]["result"][0]["uri"], "file:///tmp/base.pkl");
    assert_eq!(
        result[2]["result"][0]["range"]["start"],
        json!({"line":1,"character":0})
    );
}

#[test]
fn completion_item_resolves_schema_documentation() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/documented-base.pkl","languageId":"pkl","version":1,"text":"module Base\n/// The greeting.\ngreeting: String"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/documented-child.pkl","languageId":"pkl","version":1,"text":"amends \"./documented-base.pkl\"\ngree"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/documented-child.pkl"},"position":{"line":1,"character":4}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"completionItem/resolve","params":{"label":"greeting","kind":10,"data":{"type":"String","documentation":"The greeting."}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(
        result[0]["result"]["capabilities"]["completionProvider"]["resolveProvider"],
        true
    );
    assert_eq!(
        result[1]["result"][0]["data"]["documentation"],
        "The greeting."
    );
    assert_eq!(
        result[2]["result"]["documentation"]["value"],
        "The greeting."
    );
    assert_eq!(result[2]["result"]["detail"], "String");
}

#[test]
fn module_uri_completion_lists_neighboring_pkl_files() {
    let root = env!("CARGO_MANIFEST_DIR");
    let uri = format!("file://{root}/tests/fixtures/editing.pkl");
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pkl","version":1,"text":"import \"./sc\""}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":0,"character":12}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert!(
        result[1]["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "schema.pkl")
    );
}

#[test]
fn semantic_tokens_highlight_documentation_member_links() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/docs.pkl","languageId":"pkl","version":1,"text":"/// See [bar]\nfoo: String\nbar: Int"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":"file:///tmp/docs.pkl"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(
        result[0]["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"],
        json!(["property"])
    );
    assert_eq!(result[1]["result"]["data"], json!([0, 8, 5, 0, 1]));
}

#[test]
fn semantic_tokens_include_indented_doc_comment_column() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/indented.pkl","languageId":"pkl","version":1,"text":"class C {\n  /// See [bar]\n  foo: String\n  bar: Int\n}"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":"file:///tmp/indented.pkl"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["result"]["data"], json!([1, 10, 5, 0, 1]));
}

#[test]
fn code_action_only_removes_requested_unused_import() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/two-imports.pkl","languageId":"pkl","version":1,"text":"import \"./a.pkl\" as A\nimport \"./b.pkl\" as B\nfoo = 1\n"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction","params":{"textDocument":{"uri":"file:///tmp/two-imports.pkl"},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":21}},"context":{"diagnostics":[{"code":"unused-import","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":21}}}]}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(
        result[1]["result"][0]["edit"]["changes"]["file:///tmp/two-imports.pkl"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}})
}

#[test]
fn protocol_initialization_and_shutdown() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["id"], 1);
    assert_eq!(result[0]["result"]["capabilities"]["textDocumentSync"], 1);
    assert_eq!(result[1], json!({"jsonrpc":"2.0","id":2,"result":null}));
}

#[test]
fn document_lifecycle_and_local_completion() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/example.pkl","languageId":"pkl","version":1,"text":"firstName = 1\nfir"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/example.pkl"},"position":{"line":1,"character":3}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///tmp/example.pkl","version":2},"contentChanges":[{"text":"secondName = 2\nsec"}]}}),
        json!({"jsonrpc":"2.0","id":4,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/example.pkl"},"position":{"line":1,"character":3}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"pkl/fileContents","params":{"uri":"file:///tmp/example.pkl"}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///tmp/example.pkl"}}}),
        json!({"jsonrpc":"2.0","id":6,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["result"][0]["label"], "firstName");
    assert_eq!(result[2]["result"][0]["label"], "secondName");
    assert_eq!(result[3]["result"], "secondName = 2\nsec");
}

#[test]
fn invalid_package_request_is_not_silently_successful() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","id":9,"method":"pkl/downloadPackage","params":"not-a-package"}),
        json!({"jsonrpc":"2.0","id":10,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["error"]["code"], -32602);
}

#[test]
#[ignore = "requires public GitHub release network access"]
fn package_download_request_makes_public_module_available() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","id":2,"method":"pkl/downloadPackage","params":"package://github.com/jdx/hk/releases/download/v2.0.1/hk@2.0.1"}),
        json!({"jsonrpc":"2.0","id":3,"method":"pkl/fileContents","params":{"uri":"package://github.com/jdx/hk/releases/download/v2.0.1/hk@2.0.1#/Config.pkl"}}),
        json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["result"], Value::Null);
    assert!(
        result[2]["result"]
            .as_str()
            .unwrap()
            .contains("min_hk_version")
    );
}

#[test]
fn completion_uses_utf16_offsets_after_non_bmp_characters() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/unicode.pkl","languageId":"pkl","version":1,"text":"firstName = 1\n😀firX"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/unicode.pkl"},"position":{"line":1,"character":5}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["result"][0]["label"], "firstName");
}

#[test]
fn requests_after_shutdown_are_rejected() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","id":2,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/empty.pkl"},"position":{"line":0,"character":0}}}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[2]["error"]["code"], -32600);
}

#[test]
fn exit_without_shutdown_fails() {
    let mut process = Command::new(env!("CARGO_BIN_EXE_pkl-lsp-rs"))
        .env("PATH", "/nonexistent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let bytes = serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"exit"})).unwrap();
    let mut stdin = process.stdin.take().unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", bytes.len()).unwrap();
    stdin.write_all(&bytes).unwrap();
    drop(stdin);
    let output = process.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn truncated_lsp_header_is_not_a_clean_exit() {
    let mut process = Command::new(env!("CARGO_BIN_EXE_pkl-lsp-rs"))
        .env("PATH", "/nonexistent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process
        .stdin
        .take()
        .unwrap()
        .write_all(b"X-Test: 1\r\n")
        .unwrap();
    let output = process.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn local_amends_schema_completion_at_root_and_in_nested_object() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/schema.pkl","languageId":"pkl","version":1,"text":"module Schema\nmin_hk_version: String\nhooks: Hooks\nclass Hooks {\n  before_commit: String\n}"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/example.pkl","languageId":"pkl","version":1,"text":"amends \"./schema.pkl\"\nmin\nhooks {\n  before\n}"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/example.pkl"},"position":{"line":1,"character":3}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/example.pkl"},"position":{"line":3,"character":8}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["result"][0]["label"], "min_hk_version");
    assert_eq!(result[2]["result"][0]["label"], "before_commit");
}

#[test]
fn commented_amends_does_not_load_a_schema() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/should-not-amend.pkl","languageId":"pkl","version":1,"text":"/*\namends \"./some-schema.pkl\"\n*/\nonlyLocal = 1\nremote"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/should-not-amend.pkl"},"position":{"line":4,"character":6}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["result"], json!([]));
}

#[test]
#[ignore = "requires public GitHub release network access"]
fn public_hk_package_schema_completes_from_release() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/hk-public.pkl","languageId":"pkl","version":1,"text":"amends \"package://github.com/jdx/hk/releases/download/v2.0.1/hk@2.0.1#/Config.pkl\"\nmin_hk_v"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/hk-public.pkl"},"position":{"line":1,"character":8}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["result"][0]["label"], "min_hk_version");
}
