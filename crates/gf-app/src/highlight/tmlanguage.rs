//! Converts VS Code / TextMate `*.tmLanguage.json` grammars into syntect's
//! sublime-syntax model, so grammars from VS Code extensions can be used
//! as-is.
//!
//! Mapping:
//!
//! | tmLanguage                         | sublime-syntax                          |
//! |------------------------------------|-----------------------------------------|
//! | `patterns`                         | context `main`                          |
//! | `repository.<name>`                | context `<name>`                        |
//! | `{match, name, captures}`          | `{match, scope, captures}`              |
//! | `{begin, end, name, contentName}`  | `{match: begin, push: [meta_scope, meta_content_scope, {match: end, pop}, …]}` |
//! | `{begin, while}`                   | approximated: pop when a line stops matching `while` |
//! | `{include: "#x" / "$self" / "source.x"}` | `include: x / main / scope:source.x` |
//! | `{patterns: […]}` (group)          | anonymous context + include             |
//!
//! The sublime-syntax document is built as a JSON value and written out as
//! YAML flow syntax (JSON-compatible, but with integer capture keys, which
//! syntect requires), then loaded with syntect's YAML loader.

use serde_json::{Map, Value, json};
use syntect::parsing::SyntaxDefinition;

/// Parses a `.tmLanguage.json` grammar into a syntect definition.
pub fn load(json_text: &str) -> Result<SyntaxDefinition, String> {
    let tm: Value = serde_json::from_str(json_text).map_err(|e| format!("invalid JSON: {e}"))?;
    let doc = convert(&tm)?;
    let yaml = to_yaml(&doc);
    SyntaxDefinition::load_from_str(&yaml, true, None)
        .map_err(|e| format!("unsupported grammar: {e}"))
}

struct Converter {
    contexts: Map<String, Value>,
    anon: usize,
}

/// Builds the sublime-syntax document (as JSON).
pub fn convert(tm: &Value) -> Result<Value, String> {
    let scope = tm
        .get("scopeName")
        .and_then(Value::as_str)
        .ok_or("missing scopeName")?;
    let name = tm.get("name").and_then(Value::as_str).unwrap_or(scope);
    let extensions: Vec<String> = tm
        .get("fileTypes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|e| e.trim_start_matches('.').to_owned())
                .collect()
        })
        .unwrap_or_default();

    let mut c = Converter {
        contexts: Map::new(),
        anon: 0,
    };
    if let Some(repo) = tm.get("repository").and_then(Value::as_object) {
        for (key, rule) in repo {
            let items = if rule.get("match").is_some()
                || rule.get("begin").is_some()
                || rule.get("include").is_some()
            {
                vec![c.rule(rule)?]
            } else {
                c.patterns(rule.get("patterns"))?
            };
            c.contexts.insert(context_name(key), Value::Array(items));
        }
    }
    let main = c.patterns(tm.get("patterns"))?;
    c.contexts.insert("main".into(), Value::Array(main));

    Ok(json!({
        "name": name,
        "scope": scope,
        "file_extensions": extensions,
        "contexts": c.contexts,
    }))
}

/// Repository keys may contain characters that are not valid context names.
fn context_name(key: &str) -> String {
    let cleaned: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("r_{cleaned}")
}

impl Converter {
    fn patterns(&mut self, patterns: Option<&Value>) -> Result<Vec<Value>, String> {
        patterns
            .and_then(Value::as_array)
            .map(|list| list.iter().map(|r| self.rule(r)).collect())
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    fn rule(&mut self, r: &Value) -> Result<Value, String> {
        if let Some(inc) = r.get("include").and_then(Value::as_str) {
            return Ok(json!({ "include": include_target(inc) }));
        }
        let name = r.get("name").and_then(Value::as_str);

        if let Some(m) = r.get("match").and_then(Value::as_str) {
            let mut item = Map::new();
            item.insert("match".into(), m.into());
            if let Some(n) = name {
                item.insert("scope".into(), n.into());
            }
            if let Some(caps) = captures(r.get("captures")) {
                item.insert("captures".into(), caps);
            }
            return Ok(Value::Object(item));
        }

        if let Some(begin) = r.get("begin").and_then(Value::as_str) {
            let end_pattern = match (
                r.get("end").and_then(Value::as_str),
                r.get("while").and_then(Value::as_str),
            ) {
                (Some(end), _) => end.to_owned(),
                // Pop at the first line that no longer matches `while`.
                (None, Some(w)) => format!("^(?!{})", w.trim_start_matches('^')),
                (None, None) => return Err("begin rule without end/while".into()),
            };
            let mut push = Vec::new();
            if let Some(n) = name {
                push.push(json!({ "meta_scope": n }));
            }
            if let Some(content) = r.get("contentName").and_then(Value::as_str) {
                push.push(json!({ "meta_content_scope": content }));
            }
            let mut end = Map::new();
            end.insert("match".into(), end_pattern.into());
            if let Some(caps) = captures(r.get("endCaptures").or_else(|| r.get("captures"))) {
                end.insert("captures".into(), caps);
            }
            end.insert("pop".into(), true.into());
            push.push(Value::Object(end));
            push.extend(self.patterns(r.get("patterns"))?);

            let mut item = Map::new();
            item.insert("match".into(), begin.into());
            if let Some(caps) = captures(r.get("beginCaptures").or_else(|| r.get("captures"))) {
                item.insert("captures".into(), caps);
            }
            item.insert("push".into(), Value::Array(push));
            return Ok(Value::Object(item));
        }

        // A bare group of patterns: give it its own context.
        if r.get("patterns").is_some() {
            let items = self.patterns(r.get("patterns"))?;
            self.anon += 1;
            let ctx = format!("anon_{}", self.anon);
            self.contexts.insert(ctx.clone(), Value::Array(items));
            return Ok(json!({ "include": ctx }));
        }
        Err(format!("unsupported rule: {r}"))
    }
}

/// Writes `v` as YAML flow syntax. Identical to JSON except that numeric
/// object keys are left unquoted so YAML reads them as integers.
fn to_yaml(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let items: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let key = if !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()) {
                        k.clone()
                    } else {
                        Value::String(k.clone()).to_string()
                    };
                    format!("{key}: {}", to_yaml(v))
                })
                .collect();
            format!("{{{}}}", items.join(", "))
        }
        Value::Array(list) => format!(
            "[{}]",
            list.iter().map(to_yaml).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

fn include_target(inc: &str) -> String {
    match inc {
        "$self" | "$base" => "main".into(),
        _ => match inc.strip_prefix('#') {
            Some(name) => context_name(name),
            // `source.js` or `source.js#rule`: reference the other grammar.
            None => format!("scope:{}", inc.split('#').next().unwrap_or(inc)),
        },
    }
}

/// `{"1": {"name": "x"}}` → `{"1": "x"}` (nested capture patterns are dropped).
fn captures(caps: Option<&Value>) -> Option<Value> {
    let obj = caps?.as_object()?;
    let mut out = Map::new();
    for (k, v) in obj {
        if let Some(n) = v.get("name").and_then(Value::as_str) {
            out.insert(k.clone(), n.into());
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntect::easy::HighlightLines;
    use syntect::parsing::SyntaxSetBuilder;

    /// A tiny VS Code style grammar exercising match, begin/end, captures,
    /// repository includes, $self and groups.
    const GRAMMAR: &str = r##"{
      "name": "Toy",
      "scopeName": "source.toy",
      "fileTypes": [".toy"],
      "patterns": [
        { "include": "#comments" },
        { "include": "#strings" },
        { "match": "\\b(let|fn)\\b", "name": "keyword.control.toy" },
        { "match": "\\b(\\w+)\\s*(?=\\()", "captures": { "1": { "name": "entity.name.function.toy" } } },
        { "patterns": [ { "match": "\\b\\d+\\b", "name": "constant.numeric.toy" } ] },
        { "begin": "\\{", "end": "\\}", "patterns": [ { "include": "$self" } ] }
      ],
      "repository": {
        "comments": { "match": "#.*$", "name": "comment.line.toy" },
        "strings": {
          "begin": "\"", "end": "\"", "name": "string.quoted.double.toy",
          "patterns": [ { "match": "\\\\.", "name": "constant.character.escape.toy" } ]
        }
      }
    }"##;

    #[test]
    fn converts_and_highlights_a_vscode_grammar() {
        let def = load(GRAMMAR).expect("load");
        let mut b = SyntaxSetBuilder::new();
        b.add(def);
        let set = b.build();
        let syntax = set
            .find_syntax_by_extension("toy")
            .expect("registered by fileTypes");

        // Check scopes directly via a parse state.
        let mut state = syntect::parsing::ParseState::new(syntax);
        let ops = state
            .parse_line("let x = f(42) { \"a\\\"b\" } # done\n", &set)
            .expect("parse");
        let mut stack = syntect::parsing::ScopeStack::new();
        let mut seen = Vec::new();
        for (_, op) in ops {
            stack.apply(&op).expect("apply");
            seen.push(format!("{stack:?}"));
        }
        let all = seen.join("\n");
        for scope in [
            "keyword.control.toy",
            "entity.name.function.toy",
            "constant.numeric.toy",
            "string.quoted.double.toy",
            "constant.character.escape.toy",
            "comment.line.toy",
        ] {
            assert!(all.contains(scope), "missing {scope} in\n{all}");
        }

        // And through the highlighter API used by the app.
        let theme = syntect::highlighting::Theme::default();
        let mut h = HighlightLines::new(syntax, &theme);
        assert!(h.highlight_line("let y = 1\n", &set).is_ok());
    }

    #[test]
    fn rejects_broken_input() {
        assert!(load("{").is_err());
        assert!(
            load(r#"{"patterns": []}"#).is_err(),
            "scopeName is required"
        );
    }

    #[test]
    fn while_rules_are_approximated() {
        let g = r#"{ "scopeName": "source.q", "patterns": [ { "begin": "^>", "while": "^>", "name": "markup.quote" } ] }"#;
        assert!(load(g).is_ok());
    }
}
