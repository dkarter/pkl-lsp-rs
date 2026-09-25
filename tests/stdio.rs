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
fn unsupported_request_is_not_silently_successful() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","id":9,"method":"pkl/downloadPackage","params":"not-a-package"}),
        json!({"jsonrpc":"2.0","id":10,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["error"]["code"], -32601);
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
