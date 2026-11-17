use apifox_helper_openidea::export::Format;
use serde_json::json;
#[test]
fn yaml_quotes_ambiguous_keys_and_strings_without_changing_primitives() {
    let d = json!({"200":{"description":"成功\n第二行","example":"true","format":"2026-11-17","nullable":true,"minimum":1,"items":[{"$ref":"#/components/schemas/User"},false,null,{},[]]}});
    let yaml = Format::Yaml.render(&d).unwrap();
    assert!(yaml.contains("\"200\":"));
    assert!(yaml.contains("\"example\": \"true\""));
    assert!(yaml.contains("\"nullable\": true"));
    assert!(yaml.contains("\"minimum\": 1"));
    assert!(yaml.contains("\"description\": \"成功\\n第二行\""));
    assert!(yaml.contains("-\n"));
    assert!(yaml.contains("- {}\n"));
    assert!(yaml.contains("- []\n"));
    assert_eq!(
        Format::Json.render(&d).unwrap(),
        serde_json::to_string_pretty(&d).unwrap()
    );
    assert!(Format::parse("xml").is_err());
}
#[test]
fn yaml_unicode_line_separators_are_escaped() {
    let d = json!({"line\u{85}key":"a\u{2028}b\u{2029}c"});
    let yaml = Format::Yaml.render(&d).unwrap();
    assert!(yaml.contains("line\\u0085key"));
    assert!(yaml.contains("a\\u2028b\\u2029c"));
}
