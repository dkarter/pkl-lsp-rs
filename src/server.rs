use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

#[derive(Default)]
struct Server {
    documents: HashMap<String, String>,
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
            let bytes = serde_json::to_vec(&response).map_err(io::Error::other)?;
            write!(output, "Content-Length: {}\r\n\r\n", bytes.len())?;
            output.write_all(&bytes)?;
            output.flush()?;
        }
    }
    Ok(())
}

fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
    let mut length = None;
    loop {
        let mut line = String::new();
        if io::Read::take(&mut *input, 8192).read_line(&mut line)? == 0 {
            return if length.is_none() {
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
                "completionProvider": {"resolveProvider": false, "triggerCharacters": [".", "/", "\"", ":"]}
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
                }
                return None;
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.documents.remove(uri);
                }
                return None;
            }
            "pkl/fileContents" => self
                .documents
                .get(params["uri"].as_str().unwrap_or(""))
                .map_or(Value::Null, |text| json!(text)),
            "textDocument/completion" => {
                let text = self
                    .documents
                    .get(params["textDocument"]["uri"].as_str().unwrap_or(""));
                json!(text.map_or_else(Vec::new, |text| completion(text, &params["position"])))
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

fn completion(text: &str, position: &Value) -> Vec<Value> {
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
    let mut names: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            let (name, _) = line.trim_start().split_once('=')?;
            let name = name.trim();
            if !name.is_empty()
                && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                && name.starts_with(&word)
                && name != word
            {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|label| json!({"label":label, "kind":10}))
        .collect()
}
