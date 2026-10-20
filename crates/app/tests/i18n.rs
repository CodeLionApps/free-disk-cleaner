//! Every @tr() string in the UI and every rule rationale/hint key has a non-empty PL and EN entry.

use std::collections::HashMap;
use std::path::Path;

fn tr_literals(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("@tr(\"") {
        rest = &rest[i + 5..];
        let mut end = 0;
        let bytes = rest.as_bytes();
        while bytes[end] != b'"' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        out.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    out
}

/// msgid → msgstr, both kept in their escaped form.
fn po_entries(src: &str) -> HashMap<String, String> {
    let unquote = |l: &str, p: &str| {
        l.strip_prefix(p)
            .and_then(|s| s.strip_suffix('"'))
            .map(str::to_string)
    };
    let mut map = HashMap::new();
    let mut id = None;
    for line in src.lines() {
        if let Some(s) = unquote(line, "msgid \"") {
            id = Some(s);
        } else if let (Some(s), Some(i)) = (unquote(line, "msgstr \""), id.take()) {
            map.insert(i, s);
        }
    }
    map
}

#[test]
fn po_files_cover_every_string_in_both_languages() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut keys: Vec<String> = fdc_core::rules::all_translation_keys()
        .into_iter()
        .map(String::from)
        .collect();
    for f in std::fs::read_dir(dir.join("ui")).unwrap() {
        let p = f.unwrap().path();
        if p.extension().is_some_and(|e| e == "slint") {
            keys.extend(tr_literals(&std::fs::read_to_string(p).unwrap()));
        }
    }
    assert!(keys.len() > 100, "extraction broken: {}", keys.len());
    for lang in ["pl", "en"] {
        let po = po_entries(
            &std::fs::read_to_string(dir.join(format!("translations/{lang}/LC_MESSAGES/app.po")))
                .unwrap(),
        );
        let missing: Vec<_> = keys
            .iter()
            .filter(|k| po.get(*k).is_none_or(|s| s.is_empty()))
            .collect();
        assert!(missing.is_empty(), "{lang}.po is missing: {missing:#?}");
    }
}
