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
    assert_eq!(
        apply_format_edits("answer=42\nmessage=\"a=b\"", edits),
        "answer = 42\nmessage = \"a=b\"\n"
    );
    assert!(edits.iter().any(|edit| edit["newText"] == "\n"));
    assert!(
        !edits
            .iter()
            .any(|edit| edit["newText"].as_str().unwrap().contains("a = b"))
    );
    assert_eq!(result[2]["result"], json!([]));
}

fn formatting(source: &str, options: Value) -> Vec<Value> {
    let responses = exchange_all(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/formatter.pkl","languageId":"pkl","version":1,"text":source}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{"textDocument":{"uri":"file:///tmp/formatter.pkl"},"options":options}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    if !source.ends_with("= (") {
        let diagnostics = &responses
            .iter()
            .find(|m| m["method"] == "textDocument/publishDiagnostics")
            .unwrap()["params"]["diagnostics"];
        assert!(
            !diagnostics
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "syntax"),
            "{diagnostics}"
        );
    }
    responses.iter().find(|m| m["id"] == 2).unwrap()["result"]
        .as_array()
        .unwrap()
        .clone()
}

fn apply_format_edits(source: &str, edits: &[Value]) -> String {
    let offset = |position: &Value| {
        let row = position["line"].as_u64().unwrap() as usize;
        let col = position["character"].as_u64().unwrap() as usize;
        let start: usize = source.split_inclusive('\n').take(row).map(str::len).sum();
        let mut units = 0;
        let mut byte = start;
        for ch in source[start..].chars() {
            if units == col {
                break;
            }
            units += ch.len_utf16();
            byte += ch.len_utf8();
        }
        assert_eq!(units, col);
        byte
    };
    let mut edits: Vec<_> = edits
        .iter()
        .map(|edit| {
            (
                offset(&edit["range"]["start"]),
                offset(&edit["range"]["end"]),
                edit["newText"].as_str().unwrap(),
            )
        })
        .collect();
    edits.sort_by_key(|edit| edit.0);
    for pair in edits.windows(2) {
        assert!(pair[0].1 <= pair[1].0, "overlapping edits");
    }
    let mut result = source.to_owned();
    for (start, end, text) in edits.into_iter().rev() {
        result.replace_range(start..end, text);
    }
    result
}

fn assert_formatted(source: &str, expected: &str, options: Value) {
    let edits = formatting(source, options.clone());
    assert_eq!(apply_format_edits(source, &edits), expected);
    assert_eq!(
        formatting(expected, options),
        Vec::<Value>::new(),
        "formatting must be idempotent"
    );
}

#[test]
fn formatting_nested_classes_objects_entries_and_comments_is_idempotent() {
    let source = "class Config {\nname: String=\"a=b\"\nchild {\n// keep = { }\nvalue=1 // trailing = comment\n}\n}\nconfig = new Config {\nname=\"ok\"\nchild {\n[\"key\"]=2\n}\n}";
    let expected = "class Config {\n  name: String = \"a=b\"\n  child {\n    // keep = { }\n    value = 1 // trailing = comment\n  }\n}\nconfig = new Config {\n  name = \"ok\"\n  child {\n    [\"key\"] = 2\n  }\n}\n";
    assert_formatted(source, expected, json!({"tabSize":2,"insertSpaces":true}));
}

#[test]
fn formatting_preserves_raw_escaped_interpolated_strings_and_comment_tokens() {
    let source = "/// Keep [name] = 😀\nobj {\ntext= #\"raw = { \\\" }\"#\nother=\"escaped \\\" = \\(1 + 2)\"\n/* keep = {\n  interior unchanged\n} */\nvalue /* left */ = /* right */ 3\n}";
    let expected = "/// Keep [name] = 😀\nobj {\n  text = #\"raw = { \\\" }\"#\n  other = \"escaped \\\" = \\(1 + 2)\"\n  /* keep = {\n  interior unchanged\n} */\n  value /* left */ = /* right */ 3\n}\n";
    assert_formatted(source, expected, json!({"tabSize":2,"insertSpaces":true}));
}

#[test]
fn formatting_honors_indentation_options_and_crlf() {
    assert_formatted(
        "obj {\r\n  nested {\r\nvalue=1\r\n  }\r\n}",
        "obj {\r\n\tnested {\r\n\t\tvalue = 1\r\n\t}\r\n}\r\n",
        json!({"tabSize":4,"insertSpaces":false}),
    );
    assert_formatted(
        "obj {\n value=1\n}",
        "obj {\n    value = 1\n}\n",
        json!({"tabSize":4,"insertSpaces":true}),
    );
}

#[test]
fn formatting_uses_utf16_edit_ranges() {
    assert_formatted(
        "`😀`=\"😀=x\"",
        "`😀` = \"😀=x\"\n",
        json!({"tabSize":2,"insertSpaces":true}),
    );
}

#[test]
fn formatting_many_sibling_bodies_preserves_inline_expression_layout() {
    let source = (0..256)
        .map(|index| format!("obj{index} {{\nvalue=List(1, 2); other = (1 + 2)\n}}\n"))
        .collect::<String>();
    let expected = (0..256)
        .map(|index| format!("obj{index} {{\n  value = List(1, 2); other = (1 + 2)\n}}\n"))
        .collect::<String>();
    assert_formatted(&source, &expected, json!({"tabSize":2,"insertSpaces":true}));
}

#[test]
fn formatting_declines_excessive_input_work_and_output_expansion() {
    let too_large = format!("//{}", "x".repeat(1024 * 1024));
    let too_wide = format!("//{}", "x".repeat(16_384));
    let too_many = (0..6000)
        .map(|index| format!("value{index}=1\n"))
        .collect::<String>();
    let too_deep = format!("{}value=1\n{}", "obj {\n".repeat(140), "}\n".repeat(140));
    let too_expanded = format!(
        "{}{}{}",
        "obj {\n".repeat(40),
        (0..1700)
            .map(|index| format!("value{index}=1\n"))
            .collect::<String>(),
        "}\n".repeat(40)
    );
    for source in [too_large, too_wide, too_many, too_deep, too_expanded] {
        assert!(formatting(&source, json!({"tabSize":16,"insertSpaces":true})).is_empty());
    }
}

#[test]
fn formatting_declines_multiline_strings_continuations_and_invalid_input() {
    for source in [
        "obj {\ntext=\"\"\"\n  keep = spaces\n  \"\"\"\n}",
        "text=#\"\"\"\n  raw = spaces\n  \"\"\"#",
        "value=List(\n1,\n2\n)",
        "obj {\nfor (x in List(1)) {\nvalue=x\n}\n}",
        "obj {\nwhen (true) {\nvalue=1\n}\n}",
        "value = (",
    ] {
        assert!(
            formatting(source, json!({"tabSize":2,"insertSpaces":true})).is_empty(),
            "unsafe edits for {source}"
        );
    }
    for options in [
        json!({"tabSize":0,"insertSpaces":true}),
        json!({"tabSize":1000000,"insertSpaces":true}),
        json!({}),
    ] {
        assert!(formatting("obj {\nvalue=1\n}", options).is_empty());
    }
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
fn qualified_import_completion_uses_open_module_not_local_properties() {
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/completion-base.pkl","languageId":"pkl","version":1,"text":"module Base\nfeatureFlag: Boolean"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/completion-child.pkl","languageId":"pkl","version":1,"text":"import \"./completion-base.pkl\" as Base\nlocal = 1\nvalue = Base.fea"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///tmp/completion-child.pkl"},"position":{"line":2,"character":16}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    let items = result[1]["result"].as_array().unwrap();
    assert!(items.iter().any(|item| item["label"] == "featureFlag"));
    assert!(!items.iter().any(|item| item["label"] == "local"));
}

#[test]
fn qualified_import_completion_reads_local_fixture_from_disk() {
    let uri = format!(
        "file://{}/tests/fixtures/editing.pkl",
        env!("CARGO_MANIFEST_DIR")
    );
    let result = exchange(&[
        initialize(),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pkl","version":1,"text":"import \"./schema.pkl\" as Schema\nvalue = Schema.fea"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":18}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert!(
        result[1]["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "featureFlag")
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

fn project_initialize(name: &str) -> Value {
    let root = url::Url::from_directory_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/projects")
            .join(name),
    )
    .unwrap();
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"workspaceFolders":[{"uri":root.as_str(),"name":name}]}})
}

#[test]
fn sync_projects_resolves_literal_local_dependency_after_explicit_sync() {
    let root = project_initialize("app");
    let uri = format!(
        "{}editing.pkl",
        root["params"]["workspaceFolders"][0]["uri"]
            .as_str()
            .unwrap()
    );
    let definition = url::Url::from_file_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/projects/library/Schema.pkl"),
    )
    .unwrap();
    let result = exchange(&[
        root,
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"pkl","version":1,"text":"amends \"@library/Schema.pkl\"\nfea"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":3}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"pkl/syncProjects","params":null}),
        json!({"jsonrpc":"2.0","id":4,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":3}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"import \"@library/Schema.pkl\" as Lib\nvalue = Lib.featureFlag"}]}}),
        json!({"jsonrpc":"2.0","id":5,"method":"textDocument/definition","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":18}}}),
        json!({"jsonrpc":"2.0","id":6,"method":"textDocument/hover","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":18}}}),
        json!({"jsonrpc":"2.0","id":7,"method":"pkl/syncProjects"}),
        json!({"jsonrpc":"2.0","id":8,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["result"], json!([]));
    assert_eq!(result[2], json!({"jsonrpc":"2.0","id":3,"result":null}));
    assert_eq!(result[3]["result"][0]["label"], "featureFlag");
    assert_eq!(result[4]["result"][0]["uri"], definition.as_str());
    assert!(
        result[5]["result"]["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("synthetic local dependency")
    );
    assert_eq!(result[6], json!({"jsonrpc":"2.0","id":7,"result":null}));
}

#[test]
fn sync_projects_rejects_unsupported_declarations_and_non_file_workspaces() {
    let result = exchange(&[
        project_initialize("unsupported"),
        json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["error"]["code"], -32000);
    let result = exchange(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":[{"uri":"https://example.invalid/","name":"remote"}]}}),
        json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["error"]["code"], -32000);
}

#[test]
fn synced_dependency_members_reject_traversal_and_unknown_aliases() {
    let root = project_initialize("app");
    let uri = format!(
        "{}editing.pkl",
        root["params"]["workspaceFolders"][0]["uri"]
            .as_str()
            .unwrap()
    );
    let mut messages = vec![
        root,
        json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}),
    ];
    for (index, import) in [
        "@missing/Schema.pkl",
        "@library/../library/Schema.pkl",
        "@library/%2e%2e/library/Schema.pkl",
        "@library//Schema.pkl",
        "@library/./Schema.pkl",
        "@library/Schema.pkl?query",
    ]
    .iter()
    .enumerate()
    {
        messages.push(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"version":1,"text":format!("amends \"{import}\"\nfea")}}}));
        messages.push(json!({"jsonrpc":"2.0","id":index + 3,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":3}}}));
    }
    messages.push(json!({"jsonrpc":"2.0","id":20,"method":"shutdown"}));
    messages.push(json!({"jsonrpc":"2.0","method":"exit"}));
    let result = exchange(&messages);
    for response in &result[2..result.len() - 1] {
        assert_eq!(response["result"], json!([]));
    }
}

/// Interactive stdio client for tests that change public synthetic files between requests.
struct ProjectClient {
    process: std::process::Child,
    input: std::process::ChildStdin,
    responses: std::sync::mpsc::Receiver<Value>,
}

impl ProjectClient {
    fn new(root: &std::path::Path) -> Self {
        let mut process = Command::new(env!("CARGO_BIN_EXE_pkl-lsp-rs"))
            .env("PATH", "/nonexistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = process.stdout.take().unwrap();
        let (sender, responses) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::{BufRead, Read};
            let mut output = std::io::BufReader::new(stdout);
            loop {
                let mut header = String::new();
                if output.by_ref().take(8192).read_line(&mut header).unwrap() == 0 {
                    break;
                }
                let length: usize = header
                    .trim()
                    .strip_prefix("Content-Length: ")
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!(length <= 32 * 1024 * 1024);
                let mut blank = String::new();
                output.by_ref().take(8192).read_line(&mut blank).unwrap();
                let mut body = vec![0; length];
                output.read_exact(&mut body).unwrap();
                if sender.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            input: process.stdin.take().unwrap(),
            responses,
            process,
        };
        let uri = url::Url::from_directory_path(root).unwrap();
        client.request(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":uri.as_str()}}),
        );
        client
    }

    fn send(&mut self, message: Value) {
        let body = serde_json::to_vec(&message).unwrap();
        write!(self.input, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        self.input.write_all(&body).unwrap();
        self.input.flush().unwrap();
    }

    fn request(&mut self, message: Value) -> Value {
        let id = message["id"].clone();
        self.send(message);
        loop {
            let response = match self
                .responses
                .recv_timeout(std::time::Duration::from_secs(5))
            {
                Ok(response) => response,
                Err(error) => {
                    let _ = self.process.kill();
                    let _ = self.process.wait();
                    panic!("stdio request {id} did not complete: {error}");
                }
            };
            if response["id"] == id {
                return response;
            }
        }
    }
}

impl Drop for ProjectClient {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let _ = self.process.kill();
        } else {
            self.request(json!({"jsonrpc":"2.0","id":999,"method":"shutdown"}));
            self.send(json!({"jsonrpc":"2.0","method":"exit"}));
        }
        let status = self.process.wait().unwrap();
        if !std::thread::panicking() {
            assert!(status.success());
        }
    }
}

fn synthetic_project_root(name: &str) -> std::path::PathBuf {
    // Generated fixtures stay under ignored target/, never in user configuration.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/stdio-projects")
        .join(format!("{}-{name}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

#[test]
fn project_resync_reloads_disk_and_invalidates_failed_or_removed_mappings() {
    let root = synthetic_project_root("reload");
    let lib = root.join("library");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("PklProject"), "amends \"pkl:Project\"").unwrap();
    std::fs::write(lib.join("Schema.pkl"), "featureFlag: Boolean").unwrap();
    let project = root.join("PklProject");
    let declaration =
        "amends \"pkl:Project\"\ndependencies { [\"lib\"] = import(\"library/PklProject\") }";
    std::fs::write(&project, declaration).unwrap();
    let uri = url::Url::from_file_path(root.join("editing.pkl")).unwrap();
    let mut client = ProjectClient::new(&root);
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri.as_str(),"version":1,"text":"amends \"@lib/Schema.pkl\"\nfea"}}}));
    let sync = json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"});
    let completion = json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":uri.as_str()},"position":{"line":1,"character":3}}});
    assert_eq!(client.request(sync.clone())["result"], Value::Null);
    assert_eq!(
        client.request(completion.clone())["result"][0]["label"],
        "featureFlag"
    );
    std::fs::write(
        &project,
        "amends \"pkl:Project\"\ndependencies = read(\"env:NOT_READ\")",
    )
    .unwrap();
    assert_eq!(client.request(sync.clone())["error"]["code"], -32000);
    assert_eq!(client.request(completion.clone())["result"], json!([]));
    std::fs::write(&project, declaration).unwrap();
    client.request(sync.clone());
    assert_eq!(
        client.request(completion.clone())["result"][0]["label"],
        "featureFlag"
    );
    std::fs::write(&project, "amends \"pkl:Project\"").unwrap();
    client.request(sync);
    assert_eq!(client.request(completion)["result"], json!([]));
}

#[test]
fn project_sync_rejects_oversized_sources_and_lockfiles() {
    let root = synthetic_project_root("bounds");
    let project = root.join("PklProject");
    std::fs::write(&project, " ".repeat(1024 * 1024 + 1)).unwrap();
    let mut client = ProjectClient::new(&root);
    let sync = json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"});
    assert_eq!(client.request(sync.clone())["error"]["code"], -32000);
    std::fs::write(&project, "amends \"pkl:Project\"").unwrap();
    std::fs::write(root.join("PklProject.deps.json"), "{\"schemaVersion\":1}").unwrap();
    assert_eq!(client.request(sync)["error"]["code"], -32000);
}

#[test]
fn local_dependency_open_documents_override_disk_and_nested_projects_are_boundaries() {
    let root = project_initialize("app");
    let directory = root["params"]["workspaceFolders"][0]["uri"]
        .as_str()
        .unwrap();
    let uri = format!("{directory}editing.pkl");
    let nested_uri = format!("{directory}nested/editing.pkl");
    let dependency = url::Url::from_file_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/projects/library/Schema.pkl"),
    )
    .unwrap();
    let result = exchange(&[
        root.clone(),
        json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":dependency.as_str(),"version":1,"text":"featureFromBuffer: String"}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"version":1,"text":"import \"@library/Schema.pkl\" as Lib\nvalue = Lib.fea"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":15}}}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":nested_uri,"version":1,"text":"amends \"@library/Schema.pkl\"\nfea"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"textDocument/completion","params":{"textDocument":{"uri":nested_uri},"position":{"line":1,"character":3}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[2]["result"][0]["label"], "featureFromBuffer");
    assert_eq!(result[2]["result"].as_array().unwrap().len(), 1);
    assert_eq!(result[3]["result"], json!([]));
}

#[test]
fn local_project_subset_rejects_expressions_duplicates_and_missing_dependencies() {
    let root = synthetic_project_root("unsupported");
    let mut client = ProjectClient::new(&root);
    for source in [
        "amends \"other.pkl\"",
        "amends \"pkl:Project\"\ndependencies { [\"lib\"] = import(\"missing/PklProject\") }",
        "amends \"pkl:Project\"\ndependencies { [\"lib\"] = import(\"x/PklProject\"); [\"lib\"] = import(\"y/PklProject\") }",
        "amends \"pkl:Project\"\ndependencies { [\"lib\"] = import(\"https://example.invalid/PklProject\") }",
        "amends \"pkl:Project\"\ndependencies { [\"lib\"] = import(\"x/\\(read(\"env:NOT_READ\"))/PklProject\") }",
        "amends \"pkl:Project\"\nevaluatorSettings { modulePath { \"lib\" } }",
    ] {
        std::fs::write(root.join("PklProject"), source).unwrap();
        assert_eq!(
            client.request(json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}))["error"]["code"],
            -32000,
            "{source}"
        );
    }
}

#[cfg(unix)]
#[test]
fn dependency_symlinks_cannot_escape_and_module_reads_are_size_bounded() {
    let root = synthetic_project_root("escape");
    let library = root.join("library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(library.join("PklProject"), "amends \"pkl:Project\"").unwrap();
    std::fs::write(
        root.join("PklProject"),
        "amends \"pkl:Project\"\ndependencies { [\"lib\"] = import(\"library/PklProject\") }",
    )
    .unwrap();
    std::fs::write(root.join("outside.pkl"), "featureEscaped: Boolean").unwrap();
    std::os::unix::fs::symlink(root.join("outside.pkl"), library.join("escape.pkl")).unwrap();
    std::fs::write(
        library.join("huge.pkl"),
        format!("featureHuge: Boolean\n{}", " ".repeat(8 * 1024 * 1024)),
    )
    .unwrap();
    let uri = url::Url::from_file_path(root.join("editing.pkl")).unwrap();
    let mut client = ProjectClient::new(&root);
    client.request(json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}));
    for member in ["escape.pkl", "huge.pkl"] {
        client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri.as_str(),"version":1,"text":format!("amends \"@lib/{member}\"\nfea")}}}));
        assert_eq!(client.request(json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":uri.as_str()},"position":{"line":1,"character":3}}}))["result"], json!([]));
    }
    // Inside-directory symlinks keep their resolved protocol URI for buffer lookup.
    std::fs::write(library.join("real.pkl"), "featureDisk: Boolean").unwrap();
    std::os::unix::fs::symlink(library.join("real.pkl"), library.join("inside.pkl")).unwrap();
    let inside = url::Url::from_file_path(library.join("inside.pkl")).unwrap();
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":inside.as_str(),"version":1,"text":"featureBuffer: String"}}}));
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri.as_str(),"version":1,"text":"amends \"@lib/inside.pkl\"\nfea"}}}));
    assert_eq!(client.request(json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":uri.as_str()},"position":{"line":1,"character":3}}}))["result"][0]["label"], "featureBuffer");
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":inside.as_str(),"version":2},"contentChanges":[{"text":format!("featureTooBig: Boolean\n{}", " ".repeat(8 * 1024 * 1024))}]}}));
    assert_eq!(client.request(json!({"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":uri.as_str()},"position":{"line":1,"character":3}}}))["result"], json!([]));
}

#[cfg(unix)]
#[test]
fn project_sync_rejects_fifo_without_blocking_stdio() {
    let root = synthetic_project_root("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(root.join("PklProject"))
            .status()
            .unwrap()
            .success()
    );
    let mut client = ProjectClient::new(&root);
    assert_eq!(
        client.request(json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}))["error"]["code"],
        -32000
    );
}

#[test]
fn project_sync_enforces_workspace_and_alias_count_limits() {
    let root = synthetic_project_root("counts");
    let library = root.join("library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(library.join("PklProject"), "amends \"pkl:Project\"").unwrap();
    let mut client = ProjectClient::new(&root);
    let sync = json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"});
    for (count, expected_error) in [(64, false), (65, true)] {
        let entries: String = (0..count)
            .map(|index| format!("[\"lib{index}\"] = import(\"library/PklProject\")\n"))
            .collect();
        std::fs::write(
            root.join("PklProject"),
            format!("amends \"pkl:Project\"\ndependencies {{\n{entries}}}"),
        )
        .unwrap();
        let response = client.request(sync.clone());
        assert_eq!(response.get("error").is_some(), expected_error);
    }
    let uri = url::Url::from_directory_path(&root).unwrap();
    let folder = json!({"uri":uri.as_str(),"name":"synthetic"});
    let result = exchange(&[
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"workspaceFolders":vec![folder; 33]}}),
        sync,
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    assert_eq!(result[1]["error"]["code"], -32000);
}

#[test]
fn sync_and_dependency_buffer_changes_republish_amended_diagnostics() {
    let root = project_initialize("app");
    let uri = format!(
        "{}editing.pkl",
        root["params"]["workspaceFolders"][0]["uri"]
            .as_str()
            .unwrap()
    );
    let dependency = url::Url::from_file_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/projects/library/Schema.pkl"),
    )
    .unwrap();
    let result = exchange_all(&[
        root,
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"version":1,"text":"amends \"@library/Schema.pkl\"\nfeatureFlag = 42"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"pkl/syncProjects"}),
        json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":dependency.as_str(),"version":1,"text":"featureFlag: Int"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"shutdown"}),
        json!({"jsonrpc":"2.0","method":"exit"}),
    ]);
    let child: Vec<_> = result
        .iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .collect();
    assert_eq!(child.len(), 3);
    assert_eq!(child[0]["params"]["diagnostics"], json!([]));
    assert_eq!(
        child[1]["params"]["diagnostics"][0]["code"],
        "type-mismatch"
    );
    assert_eq!(child[2]["params"]["diagnostics"], json!([]));
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
