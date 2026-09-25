use crate::schema;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Server {
    documents: HashMap<String, String>,
    package_cache: HashMap<String, (Option<String>, Instant)>,
    outbound: Vec<Value>,
    shutdown: bool,
}

pub fn run(mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut server = Server::default();
    while let Some(message) = read_message(&mut input)? {
        if message.get("method").and_then(Value::as_str) == Some("exit") {
            return if server.shutdown {
                Ok(())
            } else {
                Err(io::Error::other("exit before shutdown"))
            };
        }
        if let Some(response) = server.handle(&message) {
            send(&mut output, &response)?;
        }
        for notification in server.outbound.drain(..) {
            send(&mut output, &notification)?;
        }
    }
    Ok(())
}

fn send(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(output, "Content-Length: {}\r\n\r\n", bytes.len())?;
    output.write_all(&bytes)?;
    output.flush()
}

fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
    let mut length = None;
    let mut saw_header = false;
    loop {
        let mut line = String::new();
        if io::Read::take(&mut *input, 8192).read_line(&mut line)? == 0 {
            return if !saw_header {
                Ok(None)
            } else {
                Err(io::Error::from(io::ErrorKind::UnexpectedEof))
            };
        }
        if !line.ends_with('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "LSP header too long",
            ));
        }
        if line.trim().is_empty() {
            break;
        }
        saw_header = true;
        if let Some(value) = line
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
        {
            length = Some(
                value
                    .1
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
            );
        }
    }
    let size = length.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    if size > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message exceeds 32 MiB",
        ));
    }
    let mut body = vec![0; size];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

impl Server {
    fn handle(&mut self, message: &Value) -> Option<Value> {
        let method = message.get("method")?.as_str()?;
        if self.shutdown {
            return message.get("id").map(|id| {
                json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32600,"message":"Server has shut down"}})
            });
        }
        let params = &message["params"];
        let result = match method {
            "initialize" => json!({"capabilities": {
                "textDocumentSync": 1,
                "completionProvider": {"resolveProvider": false, "triggerCharacters": [".", "/", "\"", ":"]},
                "hoverProvider": true,
                "definitionProvider": true,
                "semanticTokensProvider": {"legend":{"tokenTypes":["property"],"tokenModifiers":["documentation"]},"full":true,"range":false}
            }, "serverInfo": {"name": "pkl-lsp-rs", "version": env!("CARGO_PKG_VERSION")}}),
            "shutdown" => {
                self.shutdown = true;
                Value::Null
            }
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str(),
                    params["textDocument"]["text"].as_str(),
                ) {
                    self.documents.insert(uri.to_owned(), text.to_owned());
                    self.publish_diagnostics(uri, text, params["textDocument"]["version"].as_i64());
                }
                return None;
            }
            "textDocument/didChange" => {
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str(),
                    params["contentChanges"]
                        .as_array()
                        .and_then(|changes| changes.last())
                        .and_then(|change| change["text"].as_str()),
                ) && let Some(document) = self.documents.get_mut(uri)
                {
                    *document = text.to_owned();
                    self.publish_diagnostics(uri, text, params["textDocument"]["version"].as_i64());
                }
                return None;
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.documents.remove(uri);
                    self.outbound.push(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"diagnostics":[]}}));
                }
                return None;
            }
            "pkl/fileContents" => self
                .documents
                .get(params["uri"].as_str().unwrap_or(""))
                .map_or(Value::Null, |text| json!(text)),
            "textDocument/completion" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let text = self.documents.get(uri).cloned();
                json!(text.map_or_else(Vec::new, |text| {
                    let inherited = self.inherited_schema(uri, &text);
                    completion(
                        &text,
                        &params["position"],
                        inherited.as_ref().map(|(_, schema)| schema),
                    )
                }))
            }
            "textDocument/hover" | "textDocument/definition" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let line = params["position"]["line"].as_u64().unwrap_or(u64::MAX) as usize;
                let character =
                    params["position"]["character"].as_u64().unwrap_or(u64::MAX) as usize;
                let text = self.documents.get(uri).cloned();
                let symbol = text.as_deref().and_then(|text| {
                    let word = schema::word_at(text, line, character)?;
                    let inherited =
                        self.inherited_schema(uri, text)
                            .and_then(|(target, schema)| {
                                let property = schema
                                    .properties
                                    .into_iter()
                                    .find(|property| property.name == word)?;
                                Some((target, property))
                            });
                    inherited.or_else(|| {
                        schema::parse(text)?
                            .properties
                            .into_iter()
                            .find(|property| property.name == word)
                            .map(|property| (uri.to_owned(), property))
                    })
                });
                match (method, symbol) {
                    ("textDocument/hover", Some((_, property))) => {
                        json!({"contents":{"kind":"markdown","value":format!("```pkl\n{}: {}\n```",property.name,property.ty.unwrap_or_else(|| "unknown".to_owned()))}})
                    }
                    ("textDocument/definition", Some((target, property))) => {
                        json!([{"uri":target,"range":{"start":{"line":property.line,"character":property.character},"end":{"line":property.line,"character":property.character + property.name.encode_utf16().count()}}}])
                    }
                    ("textDocument/definition", None) => json!([]),
                    _ => Value::Null,
                }
            }
            "textDocument/semanticTokens/full" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let data = self
                    .documents
                    .get(uri)
                    .map_or_else(Vec::new, |text| schema::documentation_tokens(text));
                json!({"data":data})
            }
            "initialized" | "textDocument/didSave" | "$/cancelRequest" => return None,
            _ => {
                return message.get("id").map(|id| json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":format!("Method not implemented: {method}")}}));
            }
        };
        message
            .get("id")
            .map(|id| json!({"jsonrpc":"2.0", "id":id, "result":result}))
    }
}

impl Server {
    fn publish_diagnostics(&mut self, uri: &str, text: &str, version: Option<i64>) {
        let diagnostics = schema::syntax_errors(text);
        self.outbound.push(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"version":version,"diagnostics":diagnostics}}));
    }

    fn inherited_schema(&mut self, uri: &str, source: &str) -> Option<(String, schema::Schema)> {
        let amended = schema::amends_uri(source)?;
        let (target, inherited) = if amended.starts_with("package://") {
            let cached = self.package_cache.get(amended).and_then(|(value, at)| {
                (value.is_some() || at.elapsed() < Duration::from_secs(30)).then(|| value.clone())
            });
            let text = if let Some(cached) = cached {
                cached?
            } else {
                let fetched = schema::package_module(amended);
                if self.package_cache.len() >= 16 {
                    self.package_cache.clear();
                }
                self.package_cache
                    .insert(amended.to_owned(), (fetched.clone(), Instant::now()));
                fetched?
            };
            (amended.to_owned(), text)
        } else {
            let base = url::Url::parse(uri).ok()?;
            let target = base.join(amended).ok()?;
            if target.scheme() != "file" {
                return None;
            }
            let text = self.documents.get(target.as_str()).cloned().or_else(|| {
                let path = target.to_file_path().ok()?;
                let mut text = String::new();
                std::fs::File::open(path)
                    .ok()?
                    .take(schema::MAX_MODULE_BYTES + 1)
                    .read_to_string(&mut text)
                    .ok()?;
                (text.len() as u64 <= schema::MAX_MODULE_BYTES).then_some(text)
            })?;
            (target.to_string(), text)
        };
        Some((target, schema::parse(&inherited)?))
    }
}

fn completion(text: &str, position: &Value, inherited: Option<&schema::Schema>) -> Vec<Value> {
    let line = position["line"].as_u64().unwrap_or(u64::MAX) as usize;
    let column = position["character"].as_u64().unwrap_or(u64::MAX) as usize;
    let Some(current_line) = text.lines().nth(line) else {
        return Vec::new();
    };
    let mut units = 0;
    let prefix: String = current_line
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > column {
                return false;
            }
            units = next;
            true
        })
        .collect();
    let word: String = prefix
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let local = schema::parse(text);
    let mut names: Vec<&str> = local
        .iter()
        .flat_map(|schema| schema.properties.iter())
        .map(|property| property.name.as_str())
        .filter(|name| name.starts_with(&word) && *name != word)
        .collect();
    if let Some(schema) = inherited {
        let properties = if let Some(object) = schema::object_context(text, line, column) {
            schema
                .properties
                .iter()
                .find(|property| property.name == object)
                .and_then(|property| property.ty.as_ref())
                .and_then(|class| schema.classes.get(class))
                .map_or(&[][..], Vec::as_slice)
        } else {
            schema.properties.as_slice()
        };
        names.extend(
            properties
                .iter()
                .map(|property| property.name.as_str())
                .filter(|name| name.starts_with(&word) && *name != word),
        );
    }
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|label| json!({"label":label, "kind":10}))
        .collect()
}
