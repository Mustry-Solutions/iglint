//! Every offset of every embedded script and expression maps back into the
//! JSON string (or file slice) it came from, and the mapped bytes decode to
//! the same character.

mod common;

use iglint_json::EscapeStyle;
use iglint_project::script::ScriptBody;
use iglint_project::{
    EmbeddedExpression, EmbeddedScript, SourceMap, Workspace, build_model, build_view,
};
use iglint_text::{TextRange, TextSize};
use proptest::prelude::*;

/// Decodes a raw JSON string fragment.
fn decode(raw: &str) -> String {
    let doc = format!("\"{raw}\"");
    iglint_json::parse(&doc)
        .unwrap_or_else(|e| panic!("{raw:?}: {e}"))
        .as_str()
        .unwrap()
        .to_owned()
}

/// Checks one generated source against its file.
fn check(source: &str, map: &SourceMap, body: Option<(TextRange, bool)>, file: &str, what: &str) {
    let body_start = map.body_start().to_usize();
    // Header bytes map to the anchor.
    for i in 0..body_start {
        let r = map.file_range(TextRange::at(TextSize::new(i as u32), TextSize::new(1)));
        assert_eq!(r, map.anchor(), "{what}: header offset {i}");
    }
    for (i, ch) in source.get(body_start..).unwrap().char_indices() {
        let g = TextRange::at(
            TextSize::new((body_start + i) as u32),
            TextSize::of(ch.encode_utf8(&mut [0; 4]) as &str),
        );
        let r = map.file_range(g);
        let raw = r.slice(file);
        match body {
            Some((content, true)) => {
                assert!(
                    content.contains_range(r),
                    "{what}: offset {i} maps to {r} outside {content}"
                );
                assert_eq!(
                    decode(raw),
                    ch.to_string(),
                    "{what}: offset {i} raw {raw:?}"
                );
                // And back.
                assert_eq!(
                    map.generated_offset(r.start()),
                    Some(g.start()),
                    "{what}: reverse of {i}"
                );
            }
            _ => assert_eq!(raw, ch.to_string(), "{what}: offset {i}"),
        }
    }
    // The end of the body maps to the end of the string content.
    let end = map.file_offset(TextSize::of(source));
    if let Some((content, _)) = body {
        assert_eq!(end, content.end(), "{what}: end");
    }
}

fn check_script(s: &EmbeddedScript<'_>, file: &str) {
    let body = match s.body {
        ScriptBody::Json(js) => Some((js.content_range(), true)),
        ScriptBody::File { .. } => None,
    };
    check(&s.source, &s.source_map, body, file, &s.node_path);
}

fn check_expression(e: &EmbeddedExpression<'_>, file: &str) {
    check(
        e.text(),
        &e.source_map,
        Some((e.source.content_range(), true)),
        file,
        &e.node_path,
    );
}

fn check_workspace(ws: &Workspace) -> (usize, usize) {
    let (mut scripts, mut expressions) = (0, 0);
    for r in &ws.resources {
        let loaded = ws.load_resource(r.id);
        let model = build_model(ws, &loaded);
        for (file, s) in model.scripts() {
            let text = loaded.file(file).and_then(|f| f.text()).unwrap();
            check_script(s, text);
            scripts += 1;
        }
        for (file, e) in model.expressions() {
            let text = loaded.file(file).and_then(|f| f.text()).unwrap();
            check_expression(e, text);
            expressions += 1;
        }
        if let iglint_project::ModelData::EventScript(e) = &model.data
            && let Some(f) = &e.script
        {
            let text = loaded.file(f.file).and_then(|l| l.text()).unwrap();
            check_script(&f.written, text);
        }
    }
    (scripts, expressions)
}

#[test]
fn fixture_offsets_map_into_their_strings() {
    let (s, e) = check_workspace(&common::workspace("basic"));
    assert!(s > 40 && e > 15, "{s} scripts, {e} expressions");
    let (s, e) = check_workspace(&common::workspace("edge"));
    assert!(s >= 20 && e >= 20, "{s} scripts, {e} expressions");
}

fn view_with_script(body: &str, style: EscapeStyle) -> String {
    let script = iglint_json::to_json_string(body, style);
    let expr = iglint_json::to_json_string(
        &format!(
            "{{view.params.a}} + \"{}\"",
            body.replace(['"', '\\', '\n', '\r'], "")
        ),
        style,
    );
    format!(
        "{{\"root\": {{\"meta\": {{\"name\": \"root\"}}, \"type\": \"ia.container.flex\", \
         \"events\": {{\"dom\": {{\"onClick\": {{\"type\": \"script\", \"config\": {{\"script\": {script}}}}}}}}}, \
         \"propConfig\": {{\"props.text\": {{\"binding\": {{\"type\": \"expr\", \"config\": {{\"expression\": {expr}}}}}}}}}}}}}"
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn random_scripts_map_exactly(
        body in proptest::collection::vec(
            prop_oneof![
                Just("\t".to_owned()), Just("\n".to_owned()), Just("\r\n".to_owned()), Just("=".to_owned()),
                Just("<>".to_owned()), Just("&".to_owned()), Just("'".to_owned()), Just("\"".to_owned()),
                Just("\\".to_owned()), Just("é".to_owned()), Just("😀".to_owned()), Just("\u{2028}".to_owned()),
                Just("\u{1}".to_owned()), "[a-z ]{1,6}",
            ],
            0..24,
        ).prop_map(|parts| parts.concat()),
        minimal in any::<bool>(),
    ) {
        let style = if minimal { EscapeStyle::Minimal } else { EscapeStyle::Ignition };
        let text = view_with_script(&body, style);
        let root = iglint_json::parse(&text).unwrap();
        let view = build_view(&root);
        prop_assert_eq!(view.scripts.len(), 1);
        prop_assert_eq!(view.scripts[0].body_text(), body.as_str());
        check_script(&view.scripts[0], &text);
        prop_assert_eq!(view.expressions.len(), 1);
        check_expression(&view.expressions[0], &text);
    }
}
