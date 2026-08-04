//! JSON parser plugin - full-parse mode.
//!
//! Handles `.json`, `.jsonc`, `.json5` files.
//! Parses source with tree-sitter-json directly.
//!
//! Array-item identity heuristic: when an array element is an object that
//! contains a pair with key "id", "name", or "key", that value is used as the
//! node label instead of the positional index.  This produces stable diffs for
//! configuration arrays whose items are commonly reordered or inserted.

use intentumdiff_plugin_sdk::{
    cst::CstNode,
    hash::structural_hash_with_memo,
    tree::{SemanticNode, SemanticNodeBuilder},
};

wit_bindgen::generate!({
    path: "wit/plugin.wit",
    world: "parser-plugin",
});

use crate::exports::intentumdiff::plugin::parser::ExamplePair;
use crate::exports::intentumdiff::plugin::parser::Guest;
use crate::exports::intentumdiff::plugin::parser::LanguageInfoRecord;
use crate::exports::intentumdiff::plugin::parser::ParserMode;

const PLUGIN_METADATA: &str = include_str!("../plugin_metadata.info");

fn language_info_for(ids: Vec<String>) -> Vec<LanguageInfoRecord> {
    let metadata = intentumdiff_plugin_sdk::metadata::parse_plugin_metadata(PLUGIN_METADATA);
    ids.into_iter()
        .map(|language_id| {
            let info = metadata.language_or_default(&language_id);
            LanguageInfoRecord {
                language_id: info.language_id,
                language_name: info.language_name,
                language_short_name: info.language_short_name,
                monaco_language: info.monaco_language,
                default_filename: info.default_filename,
                language_file_extensions: info.language_file_extensions,
                author: metadata.author().to_string(),
                plugin_version: metadata.plugin_version().to_string(),
                last_updated: metadata.last_updated().to_string(),
            }
        })
        .collect()
}
struct JsonParser;

const TRIVIA: &[&str] = &["comment"];

const SEMANTIC_TYPES: &[&str] = &[
    "document", "object", "pair", "array", "string", "number", "true", "false", "null",
];

fn is_semantic(node_type: &str) -> bool {
    SEMANTIC_TYPES.contains(&node_type)
}

/// Strip surrounding quotes from a string literal text.
fn unquote(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Try to extract the text of the value in a `pair` node whose key matches
/// one of the given names.  Returns `None` if the pair key doesn't match.
fn pair_value_if_key(node: &CstNode, keys: &[&str]) -> Option<String> {
    // tree-sitter-json: pair → string (key), ":", value
    let mut children = node.children.iter();
    let key_node = children.next()?;
    let raw_key = key_node.text_or_empty();
    let key = unquote(raw_key);
    if !keys.contains(&key) {
        return None;
    }
    // Skip colon (unnamed) — value is the next named child
    let value_node = children.next()?;
    let raw = value_node.text_or_empty();
    Some(unquote(raw).to_string())
}

/// Scan the direct `pair` children of an `object` node and return the value
/// of the first pair whose key is "id", "name", or "key".
fn identity_label_from_object(object_node: &CstNode) -> Option<String> {
    const ID_KEYS: &[&str] = &["id", "name", "key"];
    for child in &object_node.children {
        if child.node_type == "pair" {
            if let Some(v) = pair_value_if_key(child, ID_KEYS) {
                return Some(v);
            }
        }
    }
    None
}

fn label_for(node: &CstNode) -> String {
    if node.is_leaf() {
        return unquote(node.text_or_empty()).to_string();
    }
    match node.node_type.as_str() {
        "pair" => {
            // Key is the first child (a string node)
            if let Some(key_node) = node.children.first() {
                let raw = key_node.text_or_empty();
                return unquote(raw).to_string();
            }
        }
        "object" => {
            // If the object has an identity key, surface it as the label
            if let Some(label) = identity_label_from_object(node) {
                return label;
            }
        }
        _ => {}
    }
    node.node_type.clone()
}

fn convert(
    node: &CstNode,
    id_prefix: &str,
    array_index: Option<usize>,
    memo: &mut std::collections::HashMap<usize, String>,
) -> Option<SemanticNode> {
    if TRIVIA.contains(&node.node_type.as_str()) {
        return None;
    }

    if !is_semantic(&node.node_type) {
        return None;
    }

    let children: Vec<SemanticNode> = node
        .children
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            // Track array-item index for objects directly inside arrays
            let idx = if node.node_type == "array" {
                Some(i)
            } else {
                None
            };
            convert(c, &format!("{}.{}", id_prefix, i), idx, memo)
        })
        .collect();

    let hash = structural_hash_with_memo(node, memo);

    // Compute label — for objects that are direct array items, apply the
    // identity heuristic; fall back to positional index.
    let label = if node.node_type == "object" {
        if let Some(identity) = identity_label_from_object(node) {
            identity
        } else if let Some(idx) = array_index {
            format!("[{}]", idx)
        } else {
            label_for(node)
        }
    } else {
        label_for(node)
    };

    let builder = SemanticNodeBuilder::new(
        id_prefix,
        &node.node_type,
        label,
        node.start_line,
        node.start_col,
        node.end_line,
        node.end_col,
        hash,
    )
    .children(children);

    Some(builder.build())
}

fn node_to_cst(node: tree_sitter::Node<'_>, source: &[u8]) -> CstNode {
    let children: Vec<CstNode> = (0..node.named_child_count())
        .filter_map(|i| node.named_child(i))
        .map(|child| node_to_cst(child, source))
        .collect();

    let text = if children.is_empty() {
        Some(
            node.utf8_text(source)
                .unwrap_or("")
                .chars()
                .take(4096)
                .collect(),
        )
    } else {
        None
    };

    CstNode {
        node_type: node.kind().to_string(),
        named: node.is_named(),
        text,
        start_line: node.start_position().row as u32,
        start_col: node.start_position().column as u32,
        end_line: node.end_position().row as u32,
        end_col: node.end_position().column as u32,
        children,
    }
}

fn parse_source(source: &str) -> Result<CstNode, String> {
    let mut parser = tree_sitter::Parser::new();
    let lang = tree_sitter_json::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|_| "Failed to load JSON grammar".to_string())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "Parse failed".to_string())?;
    Ok(node_to_cst(tree.root_node(), source.as_bytes()))
}

fn process_impl(source: &str) -> String {
    let root: CstNode = match parse_source(source) {
        Ok(n) => n,
        Err(e) => return format!(r#"{{"error":"{}"}}"#, e),
    };
    let mut memo: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let sem = match convert(&root, "0", None, &mut memo) {
        Some(n) => n,
        None => return r#"{"error":"Empty semantic tree"}"#.to_string(),
    };
    match serde_json::to_string(&sem) {
        Ok(s) => s,
        Err(e) => format!(r#"{{"error":"Serialisation error: {}"}}"#, e),
    }
}

impl Guest for JsonParser {
    fn get_parser_mode() -> ParserMode {
        ParserMode::FullParse
    }
    fn grammar_id() -> String {
        "json".to_string()
    }
    fn detect_language(filename: String, _content: String) -> String {
        let lower = filename.to_lowercase();
        if lower.ends_with(".json") || lower.ends_with(".jsonc") || lower.ends_with(".json5") {
            return "json".to_string();
        }
        String::new()
    }
    fn preprocess_source(source: String) -> String {
        source
    }
    fn example(_language: String) -> ExamplePair {
        ExamplePair {
            old: "{\n  \"name\": \"my-app\",\n  \"version\": \"1.0.0\",\n  \"main\": \"index.js\"\n}\n".to_string(),
            new: "{\n  \"name\": \"my-app\",\n  \"version\": \"2.0.0\",\n  \"main\": \"dist/index.js\",\n  \"scripts\": {\n    \"build\": \"tsc\",\n    \"start\": \"node dist/index.js\"\n  },\n  \"engines\": {\n    \"node\": \">=18\"\n  }\n}\n".to_string(),
        }
    }
    fn process(input: String, _language: String, _filename: String) -> String {
        process_impl(&input)
    }
    fn trivia_node_types() -> Vec<String> {
        TRIVIA.iter().map(|s| s.to_string()).collect()
    }
    fn language_ids() -> Vec<String> {
        vec!["json".to_string()]
    }
    fn language_info() -> Vec<LanguageInfoRecord> {
        language_info_for(Self::language_ids())
    }
    fn priority() -> i32 {
        0
    }
}

export!(JsonParser);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::intentumdiff::plugin::parser::Guest;
    use intentumdiff_plugin_sdk::testing as t;

    #[test]
    fn grammar_id_nonempty() {
        assert!(!JsonParser::grammar_id().is_empty());
    }

    #[test]
    fn language_ids_contain_grammar_id() {
        let gid = JsonParser::grammar_id();
        let ids = JsonParser::language_ids();
        assert!(
            ids.contains(&gid),
            "language_ids {:?} must contain {:?}",
            ids,
            gid
        );
    }

    #[test]
    fn detect_language_json() {
        assert_eq!(
            JsonParser::detect_language("config.json".to_string(), "".to_string()),
            "json"
        );
    }

    #[test]
    fn detect_language_jsonc() {
        assert_eq!(
            JsonParser::detect_language("settings.jsonc".to_string(), "".to_string()),
            "json"
        );
    }

    #[test]
    fn detect_language_json5() {
        assert_eq!(
            JsonParser::detect_language("data.json5".to_string(), "".to_string()),
            "json"
        );
    }

    #[test]
    fn detect_language_unknown() {
        let r = JsonParser::detect_language("main.rs".to_string(), "".to_string());
        assert_eq!(r.as_str(), "");
    }

    #[test]
    fn unquote_strips_double_quotes() {
        assert_eq!(unquote(r#""hello""#), "hello");
    }

    #[test]
    fn unquote_no_quotes_unchanged() {
        assert_eq!(unquote("hello"), "hello");
    }

    #[test]
    fn process_impl_empty_returns_valid_json() {
        let out = process_impl("");
        t::assert_valid_json(&out, "process(empty)");
    }

    #[test]
    fn process_impl_whitespace_returns_valid_json() {
        let out = process_impl("   \n  ");
        t::assert_valid_json(&out, "process(whitespace)");
    }
}
