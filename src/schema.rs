use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use tree_sitter::Node;

#[derive(Clone)]
pub struct Property {
    pub name: String,
    pub ty: Option<String>,
    pub documentation: Option<String>,
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

pub fn diagnostics(source: &str, inherited: Option<&Schema>) -> Vec<serde_json::Value> {
    let Some(tree) = tree(source) else {
        return Vec::new();
    };
    let root = tree.root_node();
    let mut diagnostics = syntax_errors(root, source);
    diagnostics.extend(literal_type_errors(root, source, inherited));
    diagnostics.extend(
        unused_imports_tree(root, source)
            .into_iter()
            .map(|unused| unused.diagnostic),
    );
    diagnostics
}

fn syntax_errors(root: Node<'_>, source: &str) -> Vec<serde_json::Value> {
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
    visit(root, source, &mut errors);
    errors
}

fn literal_type_errors(
    root: Node<'_>,
    source: &str,
    inherited: Option<&Schema>,
) -> Vec<serde_json::Value> {
    let mut errors = Vec::new();
    let mut cursor = root.walk();
    for node in root
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "classProperty")
    {
        let Some(property) = property(node, source) else {
            continue;
        };
        let Some(expected) = property.ty.as_deref().or_else(|| {
            inherited.and_then(|schema| {
                schema
                    .properties
                    .iter()
                    .find(|candidate| candidate.name == property.name)
                    .and_then(|candidate| candidate.ty.as_deref())
            })
        }) else {
            continue;
        };
        let mut children = node.walk();
        let literal = node.named_children(&mut children).find(|child| {
            matches!(
                child.kind(),
                "intLiteralExpr"
                    | "floatLiteralExpr"
                    | "slStringLiteralExpr"
                    | "trueLiteralExpr"
                    | "falseLiteralExpr"
            )
        });
        let Some(literal) = literal else { continue };
        let actual = match literal.kind() {
            "intLiteralExpr" => "Int",
            "floatLiteralExpr" => "Float",
            "slStringLiteralExpr" => "String",
            "trueLiteralExpr" | "falseLiteralExpr" => "Boolean",
            _ => continue,
        };
        if expected == actual || (expected == "Number" && matches!(actual, "Int" | "Float")) {
            continue;
        }
        if !matches!(expected, "Int" | "Float" | "Number" | "String" | "Boolean") {
            continue;
        }
        let start = literal.start_position();
        let end = literal.end_position();
        let position = |row: usize, byte: usize| serde_json::json!({"line":row,"character":source.lines().nth(row).and_then(|line| line.get(..byte)).map_or(0, |prefix| prefix.encode_utf16().count())});
        errors.push(serde_json::json!({
            "range":{"start":position(start.row,start.column),"end":position(end.row,end.column)},
            "severity":1,"source":"pkl-lsp-rs","code":"type-mismatch",
            "message":format!("Expected {expected}, found {actual}")
        }));
    }
    errors
}

pub struct UnusedImport {
    pub diagnostic: serde_json::Value,
    pub edit: serde_json::Value,
}

pub fn unused_imports(source: &str) -> Vec<UnusedImport> {
    let Some(tree) = tree(source) else {
        return Vec::new();
    };
    unused_imports_tree(tree.root_node(), source)
}

fn unused_imports_tree(root: Node<'_>, source: &str) -> Vec<UnusedImport> {
    let mut cursor = root.walk();
    let mut unused = Vec::new();
    let mut referenced = HashSet::new();
    fn collect_references<'a>(node: Node<'_>, source: &'a str, referenced: &mut HashSet<&'a str>) {
        if node.kind() == "importClause" {
            return;
        }
        if node.kind() == "identifier"
            && let Ok(name) = node.utf8_text(source.as_bytes())
        {
            referenced.insert(name);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect_references(child, source, referenced);
        }
    }
    collect_references(root, source, &mut referenced);
    for import in root
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "importClause")
    {
        let Some(alias) = direct(import, "identifier") else {
            continue;
        };
        let Ok(name) = alias.utf8_text(source.as_bytes()) else {
            continue;
        };
        if referenced.contains(name) {
            continue;
        }
        let line = import.start_position().row;
        let length = source
            .lines()
            .nth(line)
            .map_or(0, |text| text.encode_utf16().count());
        let declaration = serde_json::json!({"start":{"line":line,"character":0},"end":{"line":line,"character":length}});
        let remove_end = if source.lines().nth(line + 1).is_some() {
            serde_json::json!({"line":line+1,"character":0})
        } else {
            serde_json::json!({"line":line,"character":length})
        };
        unused.push(UnusedImport {
            diagnostic: serde_json::json!({"range":declaration,"severity":2,"source":"pkl-lsp-rs","code":"unused-import","message":format!("Unused import `{name}`")}),
            edit: serde_json::json!({"range":{"start":{"line":line,"character":0},"end":remove_end},"newText":""}),
        });
    }
    unused
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
                            let base = if offset == 0 {
                                source
                                    .lines()
                                    .nth(node.start_position().row)
                                    .and_then(|line| line.get(..node.start_position().column))
                                    .map_or(0, |prefix| prefix.encode_utf16().count())
                            } else {
                                0
                            };
                            let character = base + line[..start].encode_utf16().count();
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
    let documentation = direct(node, "docComment")
        .and_then(|comment| comment.utf8_text(source.as_bytes()).ok())
        .map(|comment| {
            comment
                .lines()
                .map(|line| {
                    line.trim_start()
                        .strip_prefix("///")
                        .unwrap_or(line)
                        .trim_start()
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
    let line = source.lines().nth(identifier.start_position().row)?;
    Some(Property {
        name,
        ty,
        documentation,
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

pub fn qualified_import_at(source: &str, line: usize, column: usize) -> Option<(&str, &str)> {
    let row = source.lines().nth(line)?;
    let mut units = 0;
    let byte = row
        .char_indices()
        .find_map(|(byte, c)| {
            if units >= column {
                return Some(byte);
            }
            units += c.len_utf16();
            None
        })
        .unwrap_or(row.len());
    let tree = tree(source)?;
    let point = tree_sitter::Point::new(line, byte);
    let node = tree
        .root_node()
        .named_descendant_for_point_range(point, point)?;
    if node.kind() != "identifier" {
        return None;
    }
    let access = node.parent()?;
    if access.kind() != "qualifiedAccessExpr" || direct(access, "identifier")?.id() != node.id() {
        return None;
    }
    let receiver = access.child_by_field_name("receiver")?;
    if receiver.kind() != "unqualifiedAccessExpr" {
        return None;
    }
    let alias = direct(receiver, "identifier")?
        .utf8_text(source.as_bytes())
        .ok()?;
    let name = node.utf8_text(source.as_bytes()).ok()?;
    let root = tree.root_node();
    let mut cursor = root.walk();
    for import in root
        .named_children(&mut cursor)
        .filter(|node| node.kind() == "importClause")
    {
        let Some(import_alias) =
            direct(import, "identifier").and_then(|node| node.utf8_text(source.as_bytes()).ok())
        else {
            continue;
        };
        if import_alias != alias {
            continue;
        }
        let literal = direct(import, "stringConstant")?
            .utf8_text(source.as_bytes())
            .ok()?;
        return Some((literal.strip_prefix('"')?.strip_suffix('"')?, name));
    }
    None
}

pub fn module_uri_completions(
    uri: &str,
    source: &str,
    line: usize,
    column: usize,
) -> Option<Vec<serde_json::Value>> {
    let row = source.lines().nth(line)?;
    let mut utf16 = 0;
    let end = row
        .char_indices()
        .find_map(|(byte, c)| {
            if utf16 >= column {
                return Some(byte);
            }
            utf16 += c.len_utf16();
            None
        })
        .unwrap_or(row.len());
    let prefix = row.get(..end)?.trim_start();
    let quoted = ["import", "amends", "extends"]
        .into_iter()
        .find_map(|keyword| prefix.strip_prefix(keyword)?.trim_start().strip_prefix('"'))?;
    if quoted.contains('"') {
        return None;
    }
    if quoted.starts_with("package://") {
        return Some(Vec::new());
    }
    let path = url::Url::parse(uri).ok()?.to_file_path().ok()?;
    let (folder, needle) = quoted.rsplit_once('/').unwrap_or(("", quoted));
    let directory = path.parent()?.join(folder);
    let mut items: Vec<_> = std::fs::read_dir(directory)
        .ok()?
        .take(1000)
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with(needle) || name.starts_with('.') {
                return None;
            }
            let metadata = entry.file_type().ok()?;
            if metadata.is_dir() {
                Some(serde_json::json!({"label":format!("{name}/"),"kind":19}))
            } else if metadata.is_file() && name.ends_with(".pkl") {
                Some(serde_json::json!({"label":name,"kind":17}))
            } else {
                None
            }
        })
        .collect();
    items.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
    items.truncate(100);
    Some(items)
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

/// A deliberately non-evaluating subset of PklProject, never an approximation
/// of arbitrary Pkl expressions. Reject rather than silently omit declarations.
pub fn local_project_dependencies(source: &str) -> Option<HashMap<String, String>> {
    let tree = tree(source)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let header = direct(root, "moduleHeader")?;
    let clause = direct(header, "extendsOrAmendsClause")?;
    if !clause
        .utf8_text(source.as_bytes())
        .ok()?
        .starts_with("amends")
        || direct(clause, "stringConstant")?
            .utf8_text(source.as_bytes())
            .ok()?
            != "\"pkl:Project\""
    {
        return None;
    }
    fn children(node: Node<'_>) -> Vec<Node<'_>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .filter(|child| !matches!(child.kind(), "lineComment" | "blockComment" | "docComment"))
            .collect()
    }
    fn literal<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
        let text = node.utf8_text(source.as_bytes()).ok()?;
        let value = text.strip_prefix('"')?.strip_suffix('"')?;
        (!value.contains(['"', '\\', '\n', '\r'])).then_some(value)
    }
    let mut dependencies = HashMap::new();
    let mut seen = false;
    for node in children(root) {
        if node.kind() == "moduleHeader" {
            // No module annotations, modifiers or custom inherited project.
            if children(node).len() != 1 {
                return None;
            }
            continue;
        }
        let parts = children(node);
        if node.kind() != "classProperty" || parts.len() != 2 || seen {
            return None;
        }
        if parts[0].kind() != "identifier"
            || parts[0].utf8_text(source.as_bytes()).ok()? != "dependencies"
            || parts[1].kind() != "objectBody"
        {
            return None;
        }
        seen = true;
        for entry in children(parts[1]) {
            let parts = children(entry);
            if entry.kind() != "objectEntry"
                || parts.len() != 2
                || parts[0].kind() != "slStringLiteralExpr"
                || parts[1].kind() != "importExpr"
            {
                return None;
            }
            let alias = literal(parts[0], source)?;
            if alias.is_empty()
                || !alias
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            {
                return None;
            }
            let import = children(parts[1]);
            if import.len() != 1 || import[0].kind() != "stringConstant" {
                return None;
            }
            let path = literal(import[0], source)?;
            if dependencies.len() >= 64
                || dependencies
                    .insert(alias.to_owned(), path.to_owned())
                    .is_some()
            {
                return None;
            }
        }
    }
    Some(dependencies)
}

pub fn safe_member(member: &str) -> bool {
    !member.is_empty()
        && member.len() <= 4096
        && member
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && !member.contains(['\\', '%', '?', '#', ':'])
        && !member.chars().any(char::is_control)
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

pub fn package_member(bytes: &[u8], member: &str) -> Option<String> {
    if !safe_member(member) {
        return None;
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let file = archive.by_name(member).ok()?;
    read_text(file, MAX_MODULE_BYTES)
}

pub fn read_text(reader: impl Read, limit: u64) -> Option<String> {
    let mut source = String::new();
    reader.take(limit + 1).read_to_string(&mut source).ok()?;
    (source.len() as u64 <= limit).then_some(source)
}

pub fn package_archive(uri: &str) -> Option<Vec<u8>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .new_agent();
    package_archive_with_fetch(uri, |url, limit| fetch(&agent, url, limit))
}

pub fn package_archive_with_fetch(
    uri: &str,
    fetch: impl Fn(&str, u64) -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    if uri.len() > 8192 || uri.chars().any(char::is_control) {
        return None;
    }
    let package = uri.strip_prefix("package://")?;
    if package.contains('#') {
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
    let metadata = fetch(metadata_url.as_str(), 1024 * 1024)?;
    if metadata.len() > 1024 * 1024 {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&metadata).ok()?;
    if metadata["packageZipUrl"].as_str()? != url.as_str() {
        return None;
    }
    let checksum = metadata["packageZipChecksums"]["sha256"].as_str()?;
    let bytes = fetch(url.as_str(), MAX_MODULE_BYTES)?;
    if bytes.len() as u64 > MAX_MODULE_BYTES
        || checksum.len() != 64
        || format!("{:x}", Sha256::digest(&bytes)) != checksum.to_ascii_lowercase()
    {
        return None;
    }
    Some(bytes)
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

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = "package://github.com/example/fixture/releases/download/v1/schema@1";

    #[test]
    fn package_size_limits_apply_before_verification_or_parsing() {
        assert!(
            package_archive_with_fetch(PACKAGE, |_, limit| Some(vec![b' '; limit as usize + 1]))
                .is_none()
        );
        let bytes = vec![0; MAX_MODULE_BYTES as usize + 1];
        let metadata = serde_json::to_vec(&serde_json::json!({
            "packageZipUrl":format!("https://{}.zip", PACKAGE.strip_prefix("package://").unwrap()),
            "packageZipChecksums":{"sha256":format!("{:x}", Sha256::digest(&bytes))}
        }))
        .unwrap();
        assert!(
            package_archive_with_fetch(PACKAGE, |url, _| Some(if url.ends_with(".zip") {
                bytes.clone()
            } else {
                metadata.clone()
            }))
            .is_none()
        );
    }

    #[test]
    fn decompressed_member_size_and_path_limits_remain_enforced() {
        use std::io::Write;
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "Config.pkl",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        archive
            .write_all(&vec![b' '; MAX_MODULE_BYTES as usize + 1])
            .unwrap();
        let archive = archive.finish().unwrap().into_inner();
        assert!(package_member(&archive, "Config.pkl").is_none());
        assert!(package_member(&archive, "../Config.pkl").is_none());
    }

    #[test]
    fn unsupported_hosts_do_not_invoke_transport() {
        assert!(
            package_archive_with_fetch(
                "package://localhost/releases/download/v1/schema@1",
                |_, _| panic!("trust policy broadened")
            )
            .is_none()
        );
        let oversized = format!("{PACKAGE}{}", "x".repeat(8192));
        assert!(
            package_archive_with_fetch(&oversized, |_, _| panic!("URI limit bypassed")).is_none()
        );
        assert!(
            package_archive_with_fetch(&format!("{PACKAGE}\n"), |_, _| panic!(
                "control character accepted"
            ))
            .is_none()
        );
    }
}
