use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn exchange(messages: &[Value]) -> Vec<Value> {
    let mut process = Command::new(env!("CARGO_BIN_EXE_pkl-lsp-rs"))
        // A real child process with no Java (or other executables) on PATH.
        .env("PATH", "/nonexistent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = process.stdin.take().unwrap();
        for message in messages {
            let bytes = serde_json::to_vec(message).unwrap();
            write!(stdin, "Content-Length: {}\r\n\r\n", bytes.len()).unwrap();
            stdin.write_all(&bytes).unwrap();
        }
    }
    let output = process.wait_with_output().unwrap();
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
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["error"]["code"], -32601);
}
