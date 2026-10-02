use crate::{projects, schema};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

type Fetcher = fn(&str) -> Option<Vec<u8>>;

enum Event {
    Input(io::Result<Option<Value>>),
    Download(String, Option<Vec<u8>>),
}

#[derive(Default)]
struct Server {
    documents: HashMap<String, String>,
    document_versions: HashMap<String, i64>,
    package_cache: HashMap<String, (Option<String>, Instant)>,
    archives: HashMap<String, Vec<u8>>,
    outbound: Vec<Value>,
    shutdown: bool,
    projects: projects::Projects,
    fetcher: Option<Fetcher>,
    events: Option<std::sync::mpsc::SyncSender<Event>>,
    pending: HashMap<String, Vec<Value>>,
    failures: HashMap<String, Instant>,
}

pub fn run(input: impl BufRead + Send + 'static, output: impl Write) -> io::Result<()> {
    run_with_fetcher(input, output, schema::package_archive)
}

pub fn run_with_fetcher(
    mut input: impl BufRead + Send + 'static,
    mut output: impl Write,
    fetcher: Fetcher,
) -> io::Result<()> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(4);
    let mut server = Server {
        fetcher: Some(fetcher),
        events: Some(sender.clone()),
        ..Server::default()
    };
    std::thread::spawn(move || {
        loop {
            let message = read_message(&mut input);
            let finished = !matches!(&message, Ok(Some(_)));
            if sender.send(Event::Input(message)).is_err() || finished {
                break;
            }
        }
    });
    while let Ok(event) = receiver.recv() {
        let message = match event {
            Event::Input(message) => match message? {
                Some(message) => message,
                None => {
                    server.cancel_pending("Input closed");
                    for response in server.outbound.drain(..) {
                        send(&mut output, &response)?;
                    }
                    return Ok(());
                }
            },
            Event::Download(package, archive) => {
                if !server.shutdown {
                    server.download_finished(&package, archive);
                }
                for notification in server.outbound.drain(..) {
                    send(&mut output, &notification)?;
                }
                continue;
            }
        };
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
        if let Some(package) = self.request_package(message) {
            let failed_recently = self
                .failures
                .get(&package)
                .is_some_and(|at| at.elapsed() < Duration::from_secs(30));
            if !self.archives.contains_key(&package) && !failed_recently {
                if serde_json::to_vec(message).ok()?.len() > 64 * 1024 {
                    return request_error(
                        message,
                        -32602,
                        "Package-dependent request exceeds 64 KiB",
                    );
                }
                if self.pending.values().map(Vec::len).sum::<usize>() >= 128
                    || (!self.pending.contains_key(&package) && self.pending.len() >= 4)
                {
                    return request_error(
                        message,
                        -32000,
                        "Package downloader is busy; retry later",
                    );
                }
                if let Some(waiters) = self.pending.get_mut(&package) {
                    waiters.push(message.clone());
                } else {
                    self.pending.insert(package.clone(), vec![message.clone()]);
                    let sender = self.events.as_ref().unwrap().clone();
                    let fetcher = self.fetcher.unwrap();
                    std::thread::spawn(move || {
                        let archive = fetcher(&package);
                        let _ = sender.send(Event::Download(package, archive));
                    });
                }
                return None;
            }
        }
        let params = &message["params"];
        let result = match method {
            "initialize" => {
                self.projects.initialize(params);
                json!({"capabilities": {
                "textDocumentSync": 1,
                "completionProvider": {"resolveProvider": true, "triggerCharacters": [".", "/", "\"", ":"]},
                "hoverProvider": true,
                "definitionProvider": true,
                "codeActionProvider": {"codeActionKinds":["quickfix"]},
                "documentFormattingProvider": true,
                "semanticTokensProvider": {"legend":{"tokenTypes":["property"],"tokenModifiers":["documentation"]},"full":true,"range":false}
            }, "serverInfo": {"name": "pkl-lsp-rs", "version": env!("CARGO_PKG_VERSION")}})
            }
            "pkl/syncProjects" => {
                let result = self.projects.sync();
                let dependents: Vec<_> = self
                    .documents
                    .iter()
                    .filter(|(_, text)| {
                        schema::amends_uri(text).is_some_and(|amended| amended.starts_with('@'))
                    })
                    .map(|(uri, _)| uri.clone())
                    .collect();
                for uri in dependents {
                    if let Some(text) = self.documents.get(&uri).cloned() {
                        self.publish_diagnostics(
                            &uri,
                            &text,
                            self.document_versions.get(&uri).copied(),
                        );
                    }
                }
                if let Err(reason) = result {
                    return message.get("id").map(|id| json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":reason}}));
                }
                Value::Null
            }
            "shutdown" => {
                self.shutdown = true;
                self.cancel_pending("Server shutting down");
                // Detached workers never hold stdout or delay exit. Their results are ignored.
                Value::Null
            }
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str(),
                    params["textDocument"]["text"].as_str(),
                ) {
                    self.documents.insert(uri.to_owned(), text.to_owned());
                    let version = params["textDocument"]["version"].as_i64();
                    if let Some(version) = version {
                        self.document_versions.insert(uri.to_owned(), version);
                    }
                    self.publish_diagnostics(uri, text, version);
                    self.republish_dependents(uri);
                }
                return None;
            }
            "textDocument/didChange" => {
                if let (Some(uri), Some(version), Some(changes)) = (
                    params["textDocument"]["uri"].as_str(),
                    params["textDocument"]["version"].as_i64(),
                    params["contentChanges"].as_array(),
                ) && let [change] = changes.as_slice()
                    && change.get("range").is_none()
                    && change.get("rangeLength").is_none()
                    && let Some(text) = change["text"].as_str()
                    && self.documents.contains_key(uri)
                    && self
                        .document_versions
                        .get(uri)
                        .is_some_and(|previous| version > *previous)
                {
                    self.documents.insert(uri.to_owned(), text.to_owned());
                    self.document_versions.insert(uri.to_owned(), version);
                    self.publish_diagnostics(uri, text, Some(version));
                    self.republish_dependents(uri);
                }
                return None;
            }
            "textDocument/didClose" => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.documents.remove(uri);
                    self.document_versions.remove(uri);
                    self.outbound.push(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":uri,"diagnostics":[]}}));
                    self.republish_dependents(uri);
                }
                return None;
            }
            "pkl/fileContents" => {
                let uri = params["uri"].as_str().unwrap_or("");
                let text = self.documents.get(uri).cloned().or_else(|| {
                    let (package, member) = uri.split_once("#/")?;
                    schema::package_member(self.archives.get(package)?, member)
                });
                text.map_or(Value::Null, |text| json!(text))
            }
            "pkl/downloadPackage" => {
                let uri = params.as_str().unwrap_or("");
                if !uri.starts_with("package://") || uri.contains('#') {
                    return message.get("id").map(|id| json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Expected a package URI without a fragment"}}));
                }
                if !self.archives.contains_key(uri) {
                    return request_error(message, -32000, "Could not download or verify package");
                }
                self.package_cache
                    .retain(|member, _| !member.starts_with(&format!("{uri}#/")));
                Value::Null
            }
            "textDocument/completion" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let text = self.documents.get(uri).cloned();
                json!(text.map_or_else(Vec::new, |text| {
                    let line = params["position"]["line"].as_u64().unwrap_or(u64::MAX) as usize;
                    let column =
                        params["position"]["character"].as_u64().unwrap_or(u64::MAX) as usize;
                    if let Some(items) = schema::module_uri_completions(uri, &text, line, column) {
                        return items;
                    }
                    if let Some((import, _)) = column
                        .checked_sub(1)
                        .and_then(|column| schema::qualified_import_at(&text, line, column))
                    {
                        return self.local_import_schema(uri, import).map_or_else(
                            Vec::new,
                            |(_, imported)| {
                                completion(&text, &params["position"], Some(&imported), false)
                            },
                        );
                    }
                    let inherited = self.inherited_schema(uri, &text);
                    completion(
                        &text,
                        &params["position"],
                        inherited.as_ref().map(|(_, schema)| schema),
                        true,
                    )
                }))
            }
            "completionItem/resolve" => {
                let mut item = params.clone();
                if let Some(ty) = params["data"]["type"].as_str() {
                    item["detail"] = json!(ty);
                }
                if let Some(documentation) = params["data"]["documentation"].as_str() {
                    item["documentation"] = json!({"kind":"markdown","value":documentation});
                }
                item
            }
            "textDocument/hover" | "textDocument/definition" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let line = params["position"]["line"].as_u64().unwrap_or(u64::MAX) as usize;
                let character =
                    params["position"]["character"].as_u64().unwrap_or(u64::MAX) as usize;
                let text = self.documents.get(uri).cloned();
                let symbol = text.as_deref().and_then(|text| {
                    let word = schema::word_at(text, line, character)?;
                    if let Some((import, name)) = schema::qualified_import_at(text, line, character)
                    {
                        let (target, imported) = self.local_import_schema(uri, import)?;
                        let property = imported
                            .properties
                            .into_iter()
                            .find(|property| property.name == name)?;
                        return Some((target, property));
                    }
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
                        let mut value = format!(
                            "```pkl\n{}: {}\n```",
                            property.name,
                            property.ty.unwrap_or_else(|| "unknown".to_owned())
                        );
                        if let Some(documentation) = property.documentation {
                            value.push_str("\n\n");
                            value.push_str(&documentation);
                        }
                        json!({"contents":{"kind":"markdown","value":value}})
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
            "textDocument/codeAction" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let requested = params["context"]["diagnostics"].as_array();
                let edits: Vec<Value> = self
                    .documents
                    .get(uri)
                    .filter(|_| requested.is_some())
                    .map(|text| {
                        schema::unused_imports(text)
                            .into_iter()
                            .filter(|unused| {
                                requested.is_some_and(|diagnostics| {
                                    diagnostics.iter().any(|diagnostic| {
                                        diagnostic["code"] == "unused-import"
                                            && diagnostic["range"]["start"]["line"]
                                                == unused.diagnostic["range"]["start"]["line"]
                                            && unused.diagnostic["range"]["start"]["line"]
                                                .as_u64()
                                                .is_some_and(|line| {
                                                    params["range"]["start"]["line"]
                                                        .as_u64()
                                                        .is_some_and(|start| start <= line)
                                                        && params["range"]["end"]["line"]
                                                            .as_u64()
                                                            .is_some_and(|end| end >= line)
                                                })
                                    })
                                })
                            })
                            .map(|unused| unused.edit)
                            .collect()
                    })
                    .unwrap_or_default();
                if edits.is_empty() {
                    json!([])
                } else {
                    json!([{"title":"Remove unused imports","kind":"quickfix","edit":{"changes":{uri:edits}}}])
                }
            }
            "textDocument/formatting" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                json!(self.documents.get(uri).map_or_else(Vec::new, |text| {
                    crate::formatter::format_edits(text, &params["options"])
                }))
            }
            "$/cancelRequest" => {
                for waiters in self.pending.values_mut() {
                    waiters.retain(|request| {
                        if request.get("id").is_some_and(|id| id == &params["id"]) {
                            if let Some(error) = request_error(request, -32800, "Request cancelled")
                            {
                                self.outbound.push(error);
                            }
                            false
                        } else {
                            true
                        }
                    });
                }
                return None;
            }
            "initialized" | "textDocument/didSave" => return None,
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
    fn cancel_pending(&mut self, reason: &str) {
        for (_, messages) in self.pending.drain() {
            for message in messages {
                if let Some(error) = request_error(&message, -32800, reason) {
                    self.outbound.push(error);
                }
            }
        }
    }

    fn request_package(&self, message: &Value) -> Option<String> {
        message.get("id")?;
        let params = &message["params"];
        match message["method"].as_str()? {
            "pkl/downloadPackage" => {
                let uri = params.as_str()?;
                (uri.starts_with("package://") && !uri.contains('#')).then(|| uri.to_owned())
            }
            "textDocument/completion" | "textDocument/hover" | "textDocument/definition" => {
                let uri = params["textDocument"]["uri"].as_str()?;
                let text = self.documents.get(uri)?;
                let line = params["position"]["line"].as_u64().unwrap_or(u64::MAX) as usize;
                let column = params["position"]["character"].as_u64().unwrap_or(u64::MAX) as usize;
                if message["method"] == "textDocument/completion" {
                    if schema::module_uri_completions(uri, text, line, column).is_some()
                        || column
                            .checked_sub(1)
                            .and_then(|column| schema::qualified_import_at(text, line, column))
                            .is_some()
                    {
                        return None;
                    }
                } else if schema::qualified_import_at(text, line, column).is_some() {
                    return None;
                }
                let amended = schema::amends_uri(text)?;
                if self.package_cache.get(amended).is_some_and(|(value, at)| {
                    value.is_some() || at.elapsed() < Duration::from_secs(30)
                }) {
                    return None;
                }
                let (package, member) = amended.split_once("#/")?;
                if !schema::safe_member(member) {
                    return None;
                }
                package
                    .starts_with("package://")
                    .then(|| package.to_owned())
            }
            _ => None,
        }
    }

    fn download_finished(&mut self, package: &str, archive: Option<Vec<u8>>) {
        if let Some(archive) = archive {
            if self.archives.len() >= 16 {
                self.archives.clear();
            }
            self.archives.insert(package.to_owned(), archive);
            self.failures.remove(package);
            self.package_cache
                .retain(|member, _| !member.starts_with(&format!("{package}#/")));
        } else {
            if self.failures.len() >= 16
                && let Some(oldest) = self
                    .failures
                    .iter()
                    .min_by_key(|(_, at)| **at)
                    .map(|(uri, _)| uri.clone())
            {
                self.failures.remove(&oldest);
            }
            self.failures.insert(package.to_owned(), Instant::now());
        }
        for message in self.pending.remove(package).unwrap_or_default() {
            // Re-evaluate against current documents, never a stale pre-download snapshot.
            if let Some(response) = self.handle(&message) {
                self.outbound.push(response);
            }
        }
    }

    fn local_import_schema(&self, uri: &str, import: &str) -> Option<(String, schema::Schema)> {
        let target = self.local_target(uri, import)?;
        let source = if let Some(text) = self.documents.get(target.as_str()) {
            if text.len() as u64 > schema::MAX_MODULE_BYTES {
                return None;
            }
            text.clone()
        } else {
            projects::read_bounded(&target.to_file_path().ok()?, schema::MAX_MODULE_BYTES)?
        };
        Some((target.into(), schema::parse(&source)?))
    }

    fn local_target(&self, uri: &str, import: &str) -> Option<url::Url> {
        if import.starts_with('@') {
            self.projects.resolve(uri, import)
        } else {
            let target = url::Url::parse(uri).ok()?.join(import).ok()?;
            (target.scheme() == "file").then_some(target)
        }
    }

    fn republish_dependents(&mut self, changed: &str) {
        let dependents: Vec<_> = self
            .documents
            .iter()
            .filter_map(|(uri, text)| {
                let amended = schema::amends_uri(text)?;
                let target = self.local_target(uri, amended)?;
                (uri != changed && target.as_str() == changed).then(|| {
                    (
                        uri.clone(),
                        text.clone(),
                        self.document_versions.get(uri).copied(),
                    )
                })
            })
            .collect();
        for (uri, text, version) in dependents {
            self.publish_diagnostics(&uri, &text, version);
        }
    }

    fn publish_diagnostics(&mut self, uri: &str, text: &str, version: Option<i64>) {
        let inherited =
            if schema::amends_uri(text).is_some_and(|target| !target.starts_with("package://")) {
                self.inherited_schema(uri, text)
            } else {
                None
            };
        let diagnostics = schema::diagnostics(text, inherited.as_ref().map(|(_, schema)| schema));
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
                let fetched = amended.split_once("#/").and_then(|(package, member)| {
                    self.archives
                        .get(package)
                        .and_then(|bytes| schema::package_member(bytes, member))
                });
                if self.package_cache.len() >= 16 {
                    self.package_cache.clear();
                }
                self.package_cache
                    .insert(amended.to_owned(), (fetched.clone(), Instant::now()));
                fetched?
            };
            (amended.to_owned(), text)
        } else {
            return self.local_import_schema(uri, amended);
        };
        Some((target, schema::parse(&inherited)?))
    }
}

fn request_error(message: &Value, code: i64, text: &str) -> Option<Value> {
    message
        .get("id")
        .map(|id| json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":text}}))
}

fn completion(
    text: &str,
    position: &Value,
    inherited: Option<&schema::Schema>,
    include_local: bool,
) -> Vec<Value> {
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
    let mut names = BTreeMap::new();
    let mut add = |property: &schema::Property| {
        if property.name.starts_with(&word) && property.name != word {
            names.entry(property.name.clone()).or_insert_with(|| {
                json!({
                    "label":property.name,"kind":10,
                    "data":{"type":property.ty,"documentation":property.documentation}
                })
            });
        }
    };
    if let Some(local) = include_local.then(|| schema::parse(text)).flatten() {
        for property in &local.properties {
            add(property);
        }
    }
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
        for property in properties {
            add(property);
        }
    }
    names.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_cooldown_and_oldest_entry_eviction_are_bounded() {
        let mut server = Server::default();
        let package = "package://github.com/example/fixture/releases/download/v1/schema@1";
        server
            .failures
            .insert(package.to_owned(), Instant::now() - Duration::from_secs(10));
        let request = json!({"id":1,"method":"pkl/downloadPackage","params":package});
        // No worker sender exists: an accidental immediate retry would panic.
        assert_eq!(server.handle(&request).unwrap()["error"]["code"], -32000);
        for index in 0..15 {
            server
                .failures
                .insert(format!("other{index}"), Instant::now());
        }
        server.download_finished("new", None);
        assert_eq!(server.failures.len(), 16);
        assert!(!server.failures.contains_key(package));
        assert!(server.failures.contains_key("other0"));
    }

    #[test]
    fn cached_member_remains_usable_after_archive_eviction() {
        let mut server = Server::default();
        let member =
            "package://github.com/example/fixture/releases/download/v1/schema@1#/Config.pkl";
        server.package_cache.insert(
            member.to_owned(),
            (Some("min_hk_version: String".to_owned()), Instant::now()),
        );
        server.documents.insert(
            "file:///hk.pkl".to_owned(),
            format!("amends \"{member}\"\nmin"),
        );
        let request = json!({"id":1,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///hk.pkl"},"position":{"line":1,"character":3}}});
        assert_eq!(
            server.handle(&request).unwrap()["result"][0]["label"],
            "min_hk_version"
        );
    }
}
