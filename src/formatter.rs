//! Conservative, whitespace-only formatting, independent of schema analysis.
use serde_json::{Value, json};
use tree_sitter::{Node, Tree};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 16_384;
const MAX_LINE_BYTES: usize = 16_384;

struct Edit {
    start: usize,
    end: usize,
    text: String,
}

fn parse(source: &str) -> Option<Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_pkl::LANGUAGE.into())
        .ok()?;
    parser.parse(source, None)
}

fn opaque(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "stringConstant"
            | "slStringLiteralExpr"
            | "mlStringLiteralExpr"
            | "lineComment"
            | "blockComment"
            | "docComment"
            | "shebangComment"
    )
}

struct Layout<'a> {
    source: &'a str,
    tokens: Vec<Node<'a>>,
    nodes: usize,
    edits: Vec<Edit>,
}

impl<'a> Layout<'a> {
    fn gap(&mut self, start: usize, end: usize, replacement: &str) {
        let gap = &self.source[start..end];
        // Never remove comments, semicolons, or line breaks between tokens.
        if gap.bytes().all(|b| matches!(b, b' ' | b'\t')) && gap != replacement {
            self.edits.push(Edit {
                start,
                end,
                text: replacement.to_owned(),
            });
        }
    }

    fn visit(&mut self, node: Node<'a>, depth: usize) -> Option<()> {
        self.nodes += 1;
        if depth > MAX_DEPTH || self.nodes > MAX_NODES || node.kind() == "mlStringLiteralExpr" {
            return None;
        }
        // Only known structural containers can span lines. Expression
        // continuations and future grammar additions need their own policy.
        if node.start_position().row != node.end_position().row
            && !matches!(
                node.kind(),
                "module"
                    | "moduleHeader"
                    | "clazz"
                    | "classBody"
                    | "classProperty"
                    | "objectBody"
                    | "objectProperty"
                    | "objectEntry"
                    | "objectElement"
                    | "newExpr"
                    | "amendExpr"
                    | "annotation"
                    | "docComment"
                    | "blockComment"
            )
        {
            return None;
        }
        if opaque(node) || node.child_count() == 0 {
            self.tokens.push(node);
            return Some(());
        }
        let mut cursor = node.walk();
        if node.child_count() > MAX_NODES - self.nodes {
            return None;
        }
        let children: Vec<_> = node.children(&mut cursor).collect();
        if matches!(node.kind(), "classBody" | "objectBody") {
            let close = children.last()?;
            if close.kind() != "}" {
                return None;
            }
        }
        if matches!(
            node.kind(),
            "classProperty" | "objectProperty" | "objectEntry"
        ) {
            for (index, child) in children.iter().enumerate() {
                if child.kind() == "=" && index > 0 && index + 1 < children.len() {
                    self.gap(children[index - 1].end_byte(), child.start_byte(), " ");
                    self.gap(child.end_byte(), children[index + 1].start_byte(), " ");
                }
                if child.kind() == "objectBody" && index > 0 {
                    self.gap(children[index - 1].end_byte(), child.start_byte(), " ");
                }
            }
        }
        for child in children {
            self.visit(child, depth + 1)?;
        }
        Some(())
    }
}

fn position(source: &str, lines: &[usize], byte: usize) -> Value {
    let row = lines.partition_point(|start| *start <= byte) - 1;
    json!({"line":row,"character":source[lines[row]..byte].encode_utf16().count()})
}

fn unchanged(
    before: Node<'_>,
    after: Node<'_>,
    source: &str,
    formatted: &str,
    depth: usize,
    remaining: &mut usize,
) -> bool {
    if depth > MAX_DEPTH || *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    if before.kind() != after.kind() || before.child_count() != after.child_count() {
        return false;
    }
    if opaque(before) || before.child_count() == 0 {
        return source[before.byte_range()] == formatted[after.byte_range()];
    }
    let mut left = before.walk();
    let mut right = after.walk();
    before
        .children(&mut left)
        .zip(after.children(&mut right))
        .all(|(a, b)| unchanged(a, b, source, formatted, depth + 1, remaining))
}

pub fn format_edits(source: &str, options: &Value) -> Vec<Value> {
    let Some(tab_size) = options["tabSize"]
        .as_u64()
        .filter(|size| (1..=16).contains(size))
    else {
        return Vec::new();
    };
    let Some(spaces) = options["insertSpaces"].as_bool() else {
        return Vec::new();
    };
    if source.is_empty()
        || source.len() > MAX_BYTES
        || source.split('\n').any(|line| line.len() > MAX_LINE_BYTES)
    {
        return Vec::new();
    }
    let Some(tree) = parse(source) else {
        return Vec::new();
    };
    if tree.root_node().has_error() {
        return Vec::new();
    }
    let mut layout = Layout {
        source,
        tokens: Vec::new(),
        nodes: 0,
        edits: Vec::new(),
    };
    if layout.visit(tree.root_node(), 0).is_none() {
        return Vec::new();
    }
    let unit = if spaces {
        " ".repeat(tab_size as usize)
    } else {
        "\t".to_owned()
    };
    let lines: Vec<_> = std::iter::once(0)
        .chain(
            source
                .bytes()
                .enumerate()
                .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
        )
        .collect();
    for token in &layout.tokens {
        let start = token.start_byte();
        let line_start = lines[token.start_position().row];
        let prefix = &source[line_start..start];
        // Opaque tokens protect all interior string/comment lines. Only the
        // first token on a line is indented; inline layout stays untouched.
        if !prefix.bytes().all(|b| matches!(b, b' ' | b'\t')) {
            continue;
        }
        let mut depth = 0;
        let mut parent = token.parent();
        while let Some(node) = parent {
            if matches!(node.kind(), "classBody" | "objectBody")
                && node.start_byte() < start
                && start < node.child(node.child_count() - 1).unwrap().start_byte()
            {
                depth += 1;
            }
            parent = node.parent();
        }
        let text = unit.repeat(depth);
        if prefix != text {
            layout.edits.push(Edit {
                start: line_start,
                end: start,
                text,
            });
        }
    }
    if !source.ends_with('\n') {
        layout.edits.push(Edit {
            start: source.len(),
            end: source.len(),
            text: if source.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            }
            .to_owned(),
        });
    }
    if layout.edits.is_empty() || layout.edits.len() > MAX_NODES {
        return Vec::new();
    }
    layout.edits.sort_by_key(|edit| (edit.start, edit.end));
    if layout
        .edits
        .windows(2)
        .any(|pair| pair[0].end > pair[1].start)
    {
        return Vec::new();
    }
    let removed: usize = layout.edits.iter().map(|edit| edit.end - edit.start).sum();
    let added: usize = layout.edits.iter().map(|edit| edit.text.len()).sum();
    let output_size = source.len() - removed + added;
    if output_size > MAX_BYTES {
        return Vec::new();
    }
    let mut formatted = String::with_capacity(output_size);
    let mut previous = 0;
    for edit in &layout.edits {
        formatted.push_str(&source[previous..edit.start]);
        formatted.push_str(&edit.text);
        previous = edit.end;
    }
    formatted.push_str(&source[previous..]);
    // Defense in depth: whitespace edits must preserve the parse structure
    // and every opaque/terminal token, not merely remain syntactically valid.
    let Some(check) = parse(&formatted) else {
        return Vec::new();
    };
    let mut remaining = MAX_NODES;
    if check.root_node().has_error()
        || !unchanged(
            tree.root_node(),
            check.root_node(),
            source,
            &formatted,
            0,
            &mut remaining,
        )
    {
        return Vec::new();
    }
    layout.edits.into_iter().map(|edit| json!({"range":{"start":position(source,&lines,edit.start),"end":position(source,&lines,edit.end)},"newText":edit.text})).collect()
}
