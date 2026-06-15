// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Prompt and command template interpolation helpers.
//!
//! `{key}` placeholders are substituted from a `HashMap<String, String>`.
//! `{{` is an escaped literal `{`. Unknown keys are left in place.
//!
//! - [`interpolate_agent_prompt`] — for AI agent prompt text (raw substitution).
//! - [`interpolate_command_template`] — for shell command strings (single-quote
//!   escaped substitution so untrusted values can't break out into shell syntax).

use std::collections::HashMap;

pub fn interpolate_agent_prompt(template: &str, vars: &HashMap<String, String>) -> String {
    interpolate_template(template, vars, false)
}

/// Like [`interpolate_agent_prompt`], but wraps each substituted value in
/// single-quotes so it is safe to embed in a `bash -c` command string.
///
/// Use this for **command steps** where the interpolated result is executed as
/// a shell command and the variable values may contain untrusted content
/// (e.g. ticket titles from GitHub issues).
pub fn interpolate_command_template(template: &str, vars: &HashMap<String, String>) -> String {
    interpolate_template(template, vars, true)
}

/// Shell-escape a string by wrapping it in single quotes.
/// Any embedded single quotes are replaced with `'\''` (end quote, escaped
/// literal, restart quote).
fn shell_escape_value(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn interpolate_template(
    template: &str,
    vars: &HashMap<String, String>,
    shell_escape: bool,
) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with("{{") {
            out.push('{');
            rest = &rest[2..];
            continue;
        }
        let Some(end_rel) = rest.find('}') else {
            out.push_str(rest);
            return out;
        };
        let key = &rest[1..end_rel];
        if let Some(val) = vars.get(key) {
            if shell_escape {
                out.push_str(&shell_escape_value(val));
            } else {
                out.push_str(val);
            }
        } else {
            out.push_str(&rest[..=end_rel]);
        }
        rest = &rest[end_rel + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolate_agent_prompt_substitutes_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("ticket_key".into(), "PROJ-1".into());
        vars.insert("ticket_summary".into(), "Fix login".into());
        assert_eq!(
            interpolate_agent_prompt("{ticket_key}: {ticket_summary}", &vars),
            "PROJ-1: Fix login"
        );
    }

    #[test]
    fn interpolate_agent_prompt_leaves_unknown_braces() {
        let vars = HashMap::new();
        assert_eq!(
            interpolate_agent_prompt("x {unknown} y", &vars),
            "x {unknown} y"
        );
    }

    #[test]
    fn interpolate_command_template_shell_escapes_values() {
        let mut vars = HashMap::new();
        vars.insert("ticket_key".into(), "GH-1".into());
        vars.insert("ticket_summary".into(), "Fix $(rm -rf /) bug".into());
        assert_eq!(
            interpolate_command_template("echo {ticket_key} {ticket_summary}", &vars),
            "echo 'GH-1' 'Fix $(rm -rf /) bug'"
        );
    }

    #[test]
    fn interpolate_command_template_escapes_single_quotes() {
        let mut vars = HashMap::new();
        vars.insert("val".into(), "it's broken".into());
        assert_eq!(
            interpolate_command_template("echo {val}", &vars),
            "echo 'it'\\''s broken'"
        );
    }
}
