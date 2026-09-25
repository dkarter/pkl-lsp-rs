use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use tree_sitter::Node;

#[derive(Clone)]
pub struct Property {
    pub name: String,
    pub ty: Option<String>,
    pub line: usize,
    pub character: usize,
}

pub const MAX_MODULE_BYTES: u64 = 8 * 1024 * 1024;

pub struct Schema {
    pub properties: Vec<Property>,
    pub classes: HashMap<String, Vec<Property>>,
}

pub fn parse(source: &str) -> Option<Schema> {
    let tree = tree(source)?;
    let root = tree.root_node();
    let mut schema = Schema {
        properties: Vec::new(),
        classes: HashMap::new(),
    };
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        match node.kind() {
            "classProperty" => {
                if let Some(property) = property(node, source) {
                    schema.properties.push(property);
                }
            }
            "clazz" => {
                if let (Some(name), Some(body)) =
                    (direct(node, "identifier"), direct(node, "classBody"))
                {
                    let mut members = Vec::new();
                    let mut cursor = body.walk();
                    for child in body.named_children(&mut cursor) {
                        if child.kind() == "classProperty"
                            && let Some(member) = property(child, source)
                        {
                            members.push(member);
                        }
                    }
                    schema
                        .classes
                        .insert(name.utf8_text(source.as_bytes()).ok()?.to_owned(), members);
                }
            }
            _ => {}
        }
    }
    Some(schema)
}

fn tree(source: &str) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_pkl::LANGUAGE.into())
        .ok()?;
    parser.parse(source, None)
}

pub fn syntax_errors(source: &str) -> Vec<serde_json::Value> {
    let Some(tree) = tree(source) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    fn visit(node: Node<'_>, source: &str, errors: &mut Vec<serde_json::Value>) {
        if node.is_error() || node.is_missing() {
            let start = node.start_position();
            let end = node.end_position();
            let utf16 = |row: usize, byte: usize| -> usize {
                source.lines().nth(row).map_or(0, |line| {
                    line.get(..byte).unwrap_or("").encode_utf16().count()
                })
            };
            errors.push(serde_json::json!({
                "range":{"start":{"line":start.row,"character":utf16(start.row,start.column)},"end":{"line":end.row,"character":utf16(end.row,end.column)}},
                "severity":1,"source":"pkl-lsp-rs","code":"syntax",
                "message": if node.is_missing() { format!("Missing {}",node.kind()) } else { "Invalid Pkl syntax".to_owned() }
            }));
            if node.is_error() {
                return;
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            visit(child, source, errors);
        }
    }
    visit(tree.root_node(), source, &mut errors);
    errors
}

pub fn documentation_tokens(source: &str) -> Vec<usize> {
    let Some(tree) = tree(source) else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    fn visit(node: Node<'_>, source: &str, spans: &mut Vec<(usize, usize, usize)>) {
        if node.kind() == "docComment" {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                for (offset, line) in text.lines().enumerate() {
                    let mut cursor = 0;
                    while let Some(start) = line[cursor..].find('[').map(|i| i + cursor) {
                        let Some(end) = line[start + 1..].find(']').map(|i| i + start + 1) else {
                            break;
                        };
                        let name = &line[start + 1..end];
                        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                        {
                            let character = line[..start].encode_utf16().count();
                            spans.push((
                                node.start_position().row + offset,
                                character,
                                line[start..=end].encode_utf16().count(),
                            ));
                        }
                        cursor = end + 1;
                    }
                }
            }
            return;
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            visit(child, source, spans);
        }
    }
    visit(tree.root_node(), source, &mut spans);
    let (mut last_line, mut last_column) = (0, 0);
    let mut data = Vec::new();
    for (line, column, length) in spans {
        data.extend([
            line - last_line,
            if line == last_line {
                column - last_column
            } else {
                column
            },
            length,
            0,
            1,
        ]);
        (last_line, last_column) = (line, column);
    }
    data
}

fn direct<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn property(node: Node<'_>, source: &str) -> Option<Property> {
    let identifier = direct(node, "identifier")?;
    let name = identifier.utf8_text(source.as_bytes()).ok()?.to_owned();
    let ty = direct(node, "typeAnnotation")
        .and_then(|annotation| direct(annotation, "declaredType"))
        .and_then(|declared| direct(declared, "qualifiedIdentifier"))
        .and_then(|identifier| identifier.utf8_text(source.as_bytes()).ok())
        .map(str::to_owned);
    let line = source.lines().nth(identifier.start_position().row)?;
    Some(Property {
        name,
        ty,
        line: identifier.start_position().row,
        character: line
            .get(..identifier.start_position().column)?
            .encode_utf16()
            .count(),
    })
}

pub fn word_at(source: &str, line: usize, column: usize) -> Option<&str> {
    let line = source.lines().nth(line)?;
    let mut offset = 0;
    let mut byte = None;
    for (index, c) in line.char_indices() {
        if offset >= column {
            byte = Some(index);
            break;
        }
        offset += c.len_utf16();
    }
    let byte = byte.unwrap_or(line.len());
    let identifier = |c: char| c.is_alphanumeric() || c == '_';
    let start = line[..byte]
        .rfind(|c: char| !identifier(c))
        .map_or(0, |i| i + line[i..].chars().next().unwrap().len_utf8());
    let end = line[byte..]
        .find(|c: char| !identifier(c))
        .map_or(line.len(), |i| byte + i);
    (start < end).then_some(&line[start..end])
}

pub fn amends_uri(source: &str) -> Option<&str> {
    let tree = tree(source)?;
    let header = direct(tree.root_node(), "moduleHeader")?;
    let clause = direct(header, "extendsOrAmendsClause")?;
    let content = source.get(clause.byte_range())?;
    if !content.starts_with("amends") {
        return None;
    }
    let literal = direct(clause, "stringConstant")?;
    let text = source.get(literal.byte_range())?;
    text.strip_prefix('"')?.strip_suffix('"')
}

pub fn object_context(source: &str, line: usize, column: usize) -> Option<String> {
    let prefix = source
        .lines()
        .take(line)
        .chain(source.lines().nth(line))
        .collect::<Vec<_>>();
    let mut stack = Vec::new();
    for (index, row) in prefix.iter().enumerate() {
        let row = if index == line {
            let end = row
                .char_indices()
                .scan(0, |units, (byte, c)| {
                    *units += c.len_utf16();
                    Some((byte, *units))
                })
                .take_while(|(_, units)| *units <= column)
                .last()
                .map_or(0, |(byte, _)| {
                    byte + row[byte..].chars().next().unwrap().len_utf8()
                });
            &row[..end]
        } else {
            row
        };
        for (offset, character) in row.char_indices() {
            match character {
                '{' => {
                    let preceding = row[..offset].trim_end();
                    let name = preceding
                        .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        .unwrap_or("");
                    stack.push(name.to_owned());
                }
                '}' => {
                    stack.pop();
                }
                _ => {}
            }
        }
    }
    stack.last().cloned().filter(|name| !name.is_empty())
}

pub fn package_module(uri: &str) -> Option<String> {
    const LIMIT: u64 = MAX_MODULE_BYTES;
    let (package, member) = uri.strip_prefix("package://")?.split_once("#/")?;
    if member.is_empty() || member.starts_with('/') || member.split('/').any(|part| part == "..") {
        return None;
    }
    let metadata_url = url::Url::parse(&format!("https://{package}")).ok()?;
    let url = url::Url::parse(&format!("https://{package}.zip")).ok()?;
    if url.host_str() != Some("github.com")
        || !url.path().contains("/releases/download/")
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .new_agent();
    let metadata = fetch(&agent, metadata_url.as_str(), 1024 * 1024)?;
    let metadata: serde_json::Value = serde_json::from_slice(&metadata).ok()?;
    if metadata["packageZipUrl"].as_str()? != url.as_str() {
        return None;
    }
    let checksum = metadata["packageZipChecksums"]["sha256"].as_str()?;
    let bytes = fetch(&agent, url.as_str(), LIMIT)?;
    if checksum.len() != 64
        || format!("{:x}", Sha256::digest(&bytes)) != checksum.to_ascii_lowercase()
    {
        return None;
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let file = archive.by_name(member).ok()?;
    let mut source = String::new();
    file.take(LIMIT + 1).read_to_string(&mut source).ok()?;
    if source.len() as u64 > LIMIT {
        return None;
    }
    Some(source)
}

fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Option<Vec<u8>> {
    let response = agent.get(url).call().ok()?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}
