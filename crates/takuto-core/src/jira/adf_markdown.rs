// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Convert Jira **ADF** (Atlassian Document Format) description JSON to Markdown for dashboard preview.

use serde_json::Value;

/// Turn a Jira `fields.description` value into Markdown: plain strings pass through; ADF `doc` trees are walked.
pub fn jira_description_to_markdown(description: &Value) -> String {
    if description.is_null() {
        return String::new();
    }
    if let Some(s) = description.as_str() {
        return s.trim().to_string();
    }
    let mut out = String::new();
    match description.get("type").and_then(|t| t.as_str()) {
        Some("doc") => {
            if let Some(content) = description.get("content").and_then(|c| c.as_array()) {
                for child in content {
                    append_block_markdown(child, &mut out, 0);
                }
            }
        }
        _ => append_block_markdown(description, &mut out, 0),
    }
    trim_trailing_newlines(&out).to_string()
}

fn trim_trailing_newlines(s: &str) -> &str {
    s.trim_end_matches(['\n', '\r', ' '])
}

fn append_block_markdown(node: &Value, out: &mut String, list_depth: u8) {
    let t = node.get("type").and_then(|x| x.as_str()).unwrap_or("");
    match t {
        "paragraph" => {
            append_inline_content(node, out);
            out.push_str("\n\n");
        }
        "heading" => {
            let level = node
                .get("attrs")
                .and_then(|a| a.get("level"))
                .and_then(|l| l.as_u64())
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            out.push_str(&"#".repeat(level));
            out.push(' ');
            append_inline_content(node, out);
            out.push_str("\n\n");
        }
        "bulletList" => {
            if let Some(items) = node.get("content").and_then(|c| c.as_array()) {
                for item in items {
                    append_list_item_markdown(item, out, list_depth, false, 0);
                }
            }
        }
        "orderedList" => {
            if let Some(items) = node.get("content").and_then(|c| c.as_array()) {
                for (i, item) in items.iter().enumerate() {
                    append_list_item_markdown(item, out, list_depth, true, i + 1);
                }
            }
        }
        "codeBlock" => {
            let lang = node
                .get("attrs")
                .and_then(|a| a.get("language"))
                .and_then(|l| l.as_str())
                .unwrap_or("");
            out.push_str("```");
            out.push_str(lang);
            out.push('\n');
            if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
                for c in content {
                    if let Some(text) = c.get("text").and_then(|x| x.as_str()) {
                        out.push_str(text);
                    }
                }
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
        "blockquote" => {
            if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
                for c in content {
                    let mut inner = String::new();
                    append_block_markdown(c, &mut inner, list_depth);
                    for line in inner.lines() {
                        if line.is_empty() {
                            out.push_str(">\n");
                        } else {
                            out.push('>');
                            if !line.starts_with('>') {
                                out.push(' ');
                            }
                            out.push_str(line);
                            out.push('\n');
                        }
                    }
                }
            }
            out.push('\n');
        }
        "rule" | "horizontalRule" => {
            out.push_str("---\n\n");
        }
        "panel" | "expand" | "nestedExpand" => {
            if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
                for c in content {
                    append_block_markdown(c, out, list_depth);
                }
            }
        }
        "mediaGroup" | "mediaSingle" | "extension" | "multiBodiedExtension" => {
            if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
                for c in content {
                    append_block_markdown(c, out, list_depth);
                }
            }
        }
        _ => {
            if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
                for c in content {
                    append_block_markdown(c, out, list_depth);
                }
            } else if let Some(text) = node.get("text").and_then(|x| x.as_str()) {
                let marks = node
                    .get("marks")
                    .and_then(|m| m.as_array())
                    .map(|a| a.as_slice())
                    .unwrap_or(&[]);
                out.push_str(&apply_text_marks(text, marks));
            }
        }
    }
}

fn append_list_item_markdown(
    item: &Value,
    out: &mut String,
    list_depth: u8,
    ordered: bool,
    index: usize,
) {
    let indent = "    ".repeat(list_depth as usize);
    if let Some("listItem") = item.get("type").and_then(|t| t.as_str())
        && let Some(content) = item.get("content").and_then(|c| c.as_array())
    {
        let mut first = true;
        for block in content {
            let bt = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if first {
                out.push_str(&indent);
                if ordered {
                    out.push_str(&format!("{index}. "));
                } else {
                    out.push_str("- ");
                }
                first = false;
                match bt {
                    "paragraph" => {
                        append_inline_content(block, out);
                        out.push('\n');
                    }
                    "bulletList" | "orderedList" => {
                        out.push('\n');
                        append_block_markdown(block, out, list_depth + 1);
                    }
                    _ => {
                        let mut tmp = String::new();
                        append_block_markdown(block, &mut tmp, list_depth + 1);
                        let t = tmp.trim_end();
                        if !t.is_empty() {
                            out.push_str(t);
                        }
                        out.push('\n');
                    }
                }
            } else {
                out.push_str(&indent);
                out.push_str("  ");
                append_block_markdown(block, out, list_depth + 1);
            }
        }
    }
    if !out.ends_with("\n\n") && out.ends_with('\n') {
        out.push('\n');
    }
}

fn append_inline_content(block: &Value, out: &mut String) {
    let Some(content) = block.get("content").and_then(|c| c.as_array()) else {
        return;
    };
    for c in content {
        if let Some(text) = c.get("text").and_then(|t| t.as_str()) {
            let marks = c
                .get("marks")
                .and_then(|m| m.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]);
            out.push_str(&apply_text_marks(text, marks));
        } else {
            let ct = c.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match ct {
                "hardBreak" => out.push_str("  \n"),
                "emoji" => {
                    if let Some(short) = c
                        .get("attrs")
                        .and_then(|a| a.get("shortName"))
                        .and_then(|s| s.as_str())
                    {
                        out.push(':');
                        out.push_str(short);
                        out.push(':');
                    }
                }
                "mention" => {
                    if let Some(id) = c
                        .get("attrs")
                        .and_then(|a| a.get("text"))
                        .or_else(|| c.get("attrs").and_then(|a| a.get("id")))
                        .and_then(|x| x.as_str())
                    {
                        out.push('@');
                        out.push_str(id);
                    }
                }
                "inlineCard" | "blockCard" => {
                    if let Some(url) = c
                        .get("attrs")
                        .and_then(|a| a.get("url"))
                        .and_then(|u| u.as_str())
                    {
                        out.push_str(&format!("<{url}>"));
                    }
                }
                "date" => {
                    if let Some(ts) = c
                        .get("attrs")
                        .and_then(|a| a.get("timestamp"))
                        .and_then(|t| t.as_i64())
                    {
                        out.push_str(&format!("`{ts}`"));
                    }
                }
                _ => {
                    if let Some(content) = c.get("content").and_then(|x| x.as_array()) {
                        let wrapper = serde_json::json!({ "content": content });
                        append_inline_content(&wrapper, out);
                    }
                }
            }
        }
    }
}

fn apply_text_marks(text: &str, marks: &[Value]) -> String {
    let mut s = text.to_string();
    for m in marks {
        let mt = m.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match mt {
            "strong" => {
                s = format!("**{s}**");
            }
            "em" => {
                s = format!("*{s}*");
            }
            "code" => {
                s = format!("`{}`", escape_inline_code(&s));
            }
            "strike" | "subsup" => {
                s = format!("~~{s}~~");
            }
            "underline" => {
                // No native markdown; keep text
            }
            "link" => {
                let href = m
                    .get("attrs")
                    .and_then(|a| a.get("href"))
                    .and_then(|h| h.as_str())
                    .unwrap_or("");
                let label = escape_link_destination_safe(&s);
                if href.is_empty() {
                    s = label;
                } else {
                    s = format!("[{label}]({href})");
                }
            }
            "textColor" | "backgroundColor" => {}
            _ => {}
        }
    }
    s
}

fn escape_inline_code(s: &str) -> String {
    s.replace('`', "'")
}

fn escape_link_destination_safe(s: &str) -> String {
    s.replace('[', "\\[").replace(']', "\\]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_string_passthrough() {
        assert_eq!(
            jira_description_to_markdown(&json!("Hello **not** bold")),
            "Hello **not** bold"
        );
    }

    #[test]
    fn simple_doc_paragraph() {
        let adf = json!({
            "type": "doc",
            "version": 1,
            "content": [
                {
                    "type": "paragraph",
                    "content": [{ "type": "text", "text": "Hello " }, { "type": "text", "text": "world", "marks": [{ "type": "strong" }] }]
                }
            ]
        });
        let md = jira_description_to_markdown(&adf);
        assert!(md.contains("**world**"), "got {md:?}");
    }

    #[test]
    fn heading_and_bullet() {
        let adf = json!({
            "type": "doc",
            "version": 1,
            "content": [
                { "type": "heading", "attrs": { "level": 2 }, "content": [{ "type": "text", "text": "Title" }] },
                {
                    "type": "bulletList",
                    "content": [
                        { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "One" }] }] },
                        { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Two" }] }] }
                    ]
                }
            ]
        });
        let md = jira_description_to_markdown(&adf);
        assert!(md.contains("## Title"));
        assert!(md.contains("- One"));
        assert!(md.contains("- Two"));
    }

    #[test]
    fn null_description_is_empty() {
        assert_eq!(jira_description_to_markdown(&Value::Null), "");
    }

    #[test]
    fn code_block_fenced_with_language() {
        let adf = json!({
            "type": "doc",
            "content": [{
                "type": "codeBlock",
                "attrs": { "language": "rust" },
                "content": [{ "type": "text", "text": "fn main() {}" }]
            }]
        });
        let md = jira_description_to_markdown(&adf);
        assert!(md.contains("```rust\nfn main() {}"), "got {md:?}");
        assert!(md.contains("```"));
    }

    #[test]
    fn blockquote_prefixes_lines() {
        let adf = json!({
            "type": "doc",
            "content": [{
                "type": "blockquote",
                "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "quoted" }] }]
            }]
        });
        let md = jira_description_to_markdown(&adf);
        assert!(md.contains("> quoted"), "got {md:?}");
    }

    #[test]
    fn horizontal_rules_render_as_dashes() {
        for kind in ["rule", "horizontalRule"] {
            let adf = json!({ "type": "doc", "content": [{ "type": kind }] });
            assert_eq!(jira_description_to_markdown(&adf), "---");
        }
    }

    #[test]
    fn ordered_list_numbers_items() {
        let adf = json!({
            "type": "doc",
            "content": [{
                "type": "orderedList",
                "content": [
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "first" }] }] },
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "second" }] }] }
                ]
            }]
        });
        let md = jira_description_to_markdown(&adf);
        assert!(md.contains("1. first"), "got {md:?}");
        assert!(md.contains("2. second"), "got {md:?}");
    }

    #[test]
    fn nested_bullet_list_is_indented() {
        let adf = json!({
            "type": "doc",
            "content": [{
                "type": "bulletList",
                "content": [{
                    "type": "listItem",
                    "content": [
                        { "type": "paragraph", "content": [{ "type": "text", "text": "outer" }] },
                        { "type": "bulletList", "content": [
                            { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "inner" }] }] }
                        ]}
                    ]
                }]
            }]
        });
        let md = jira_description_to_markdown(&adf);
        assert!(md.contains("- outer"), "got {md:?}");
        assert!(md.contains("    - inner"), "nested item must be indented; got {md:?}");
    }

    #[test]
    fn panel_unwraps_inner_content() {
        let adf = json!({
            "type": "doc",
            "content": [{
                "type": "panel",
                "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "noted" }] }]
            }]
        });
        assert_eq!(jira_description_to_markdown(&adf), "noted");
    }

    #[test]
    fn heading_level_clamps_to_six() {
        let adf = json!({
            "type": "doc",
            "content": [{ "type": "heading", "attrs": { "level": 9 }, "content": [{ "type": "text", "text": "deep" }] }]
        });
        assert!(jira_description_to_markdown(&adf).starts_with("###### deep"));
    }

    #[test]
    fn inline_marks_em_strike_and_code_escaping() {
        let para = |marks: serde_json::Value, text: &str| {
            json!({
                "type": "doc",
                "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": text, "marks": marks }] }]
            })
        };
        assert_eq!(
            jira_description_to_markdown(&para(json!([{ "type": "em" }]), "x")),
            "*x*"
        );
        assert_eq!(
            jira_description_to_markdown(&para(json!([{ "type": "strike" }]), "x")),
            "~~x~~"
        );
        // Backticks inside inline code are downgraded to single quotes.
        assert_eq!(
            jira_description_to_markdown(&para(json!([{ "type": "code" }]), "a`b")),
            "`a'b`"
        );
    }

    #[test]
    fn link_mark_renders_destination_and_escapes_label() {
        let adf = json!({
            "type": "doc",
            "content": [{ "type": "paragraph", "content": [{
                "type": "text", "text": "[click]",
                "marks": [{ "type": "link", "attrs": { "href": "https://x.test" } }]
            }] }]
        });
        // Brackets in the label are escaped; the href is kept verbatim.
        assert_eq!(
            jira_description_to_markdown(&adf),
            "[\\[click\\]](https://x.test)"
        );
    }

    #[test]
    fn link_mark_without_href_keeps_label_only() {
        let adf = json!({
            "type": "doc",
            "content": [{ "type": "paragraph", "content": [{
                "type": "text", "text": "bare", "marks": [{ "type": "link", "attrs": {} }]
            }] }]
        });
        assert_eq!(jira_description_to_markdown(&adf), "bare");
    }

    #[test]
    fn inline_nodes_hardbreak_emoji_mention_card_and_date() {
        let adf = json!({
            "type": "doc",
            "content": [{ "type": "paragraph", "content": [
                { "type": "text", "text": "a" },
                { "type": "hardBreak" },
                { "type": "emoji", "attrs": { "shortName": "smile" } },
                { "type": "mention", "attrs": { "text": "alice" } },
                { "type": "inlineCard", "attrs": { "url": "https://card.test" } },
                { "type": "date", "attrs": { "timestamp": 123 } }
            ] }]
        });
        let md = jira_description_to_markdown(&adf);
        assert!(md.contains("a  \n"), "hardBreak → two-space newline; got {md:?}");
        assert!(md.contains(":smile:"));
        assert!(md.contains("@alice"));
        assert!(md.contains("<https://card.test>"));
        assert!(md.contains("`123`"));
    }

    #[test]
    fn mention_falls_back_to_id_when_no_text() {
        let adf = json!({
            "type": "doc",
            "content": [{ "type": "paragraph", "content": [
                { "type": "mention", "attrs": { "id": "u-42" } }
            ] }]
        });
        assert_eq!(jira_description_to_markdown(&adf), "@u-42");
    }

    #[test]
    fn escape_helpers_are_targeted() {
        assert_eq!(escape_inline_code("a`b`c"), "a'b'c");
        assert_eq!(escape_link_destination_safe("[x]"), "\\[x\\]");
    }
}
