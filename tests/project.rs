use apifox_helper_openidea::{
    openapi::{Scope, generate_with_project},
    project::{ProjectContext, ProjectSettings},
    source,
};
use serde_json::json;
#[test]
fn yaml_properties_defaults_and_profiles_do_not_guess_runtime_values() {
    let mut s = ProjectSettings::default();
    s.config("application.yml","spring:\n  profiles:\n    active: '@profiles.active@'\nsse:\n  path: /resource/sse\n---\nspring:\n  config:\n    activate:\n      on-profile: prod\nsse:\n  path: /production\n").unwrap();
    s.config(
        "other/application.properties",
        "prefix=/api\npath=${prefix}/events",
    )
    .unwrap();
    s.config("src/test/resources/application.yml", "sse:\n  path: /test")
        .unwrap();
    let c = s.finish();
    assert_eq!(c.route("${sse.path}/close").unwrap(), "/resource/sse/close");
    assert_eq!(c.route("${path}").unwrap(), "/api/events");
    assert_eq!(
        c.route("${missing:${sse.path}}/connect").unwrap(),
        "/resource/sse/connect"
    );
    assert_eq!(c.route("${missing:/fallback}").unwrap(), "/fallback");
    assert!(c.route("${MISSING_ENV}").is_err());
}
#[test]
fn conflicts_cycles_and_broken_placeholders_are_errors() {
    let mut s = ProjectSettings::default();
    s.config("a/application.properties", "route=/one\na=${b}\nb=${a}")
        .unwrap();
    s.config("b/application.yml", "route: /two").unwrap();
    let c = s.finish();
    assert!(c.route("${route}").unwrap_err().contains("冲突"));
    assert!(c.route("${a}").unwrap_err().contains("循环"));
    assert!(c.route("${a").is_err());
}
#[test]
fn hutool_tree_preserves_generic_ids_custom_label_recursion_and_dynamic_fields() {
    let mut s = ProjectSettings::default();
    s.java(r#"import cn.hutool.core.lang.tree.TreeNodeConfig; class Utils{static final Object CONFIG=TreeNodeConfig.DEFAULT_CONFIG.setNameKey("label");}"#);
    let code = r#"package p; import cn.hutool.core.lang.tree.Tree; import java.util.Properties;
 @RestController class Api { @GetMapping("/a") public R<List<Tree<Long>>> a(){return null;} @GetMapping("/b") public R<Tree<String>> b(){return null;} @GetMapping("/properties") public Properties props(){return null;} } class R<T>{private T data;}"#;
    let r = generate_with_project(
        &source::parse("Api.java", code),
        &Scope::default(),
        "测试",
        &s.finish(),
    );
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let schemas = &r.document["components"]["schemas"];
    assert_eq!(schemas["TreeLong"]["properties"]["id"]["format"], "int64");
    assert_eq!(schemas["TreeString"]["properties"]["id"]["type"], "string");
    assert_eq!(schemas["TreeLong"]["properties"]["label"]["type"], "string");
    assert!(schemas["TreeLong"]["properties"].get("name").is_none());
    assert_eq!(
        schemas["TreeLong"]["properties"]["children"]["items"]["$ref"],
        "#/components/schemas/TreeLong"
    );
    assert_eq!(schemas["TreeLong"]["additionalProperties"], json!({}));
    assert_eq!(
        schemas["RListTreeLong"]["properties"]["data"]["items"]["$ref"],
        "#/components/schemas/TreeLong"
    );
    assert_eq!(
        r.document["paths"]["/properties"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["additionalProperties"],
        json!({})
    );
}
#[test]
fn local_tree_is_not_a_hutool_map() {
    let classes = source::parse(
        "Api.java",
        r#"@RestController class Api{ @GetMapping("/x") public Tree<Long> x(){return null;} } class Tree<T>{private T custom;}"#,
    );
    let r = generate_with_project(
        &classes,
        &Scope::default(),
        "测试",
        &ProjectContext::default(),
    );
    assert!(r.warnings.is_empty());
    let tree = &r.document["components"]["schemas"]["TreeLong"];
    assert!(tree["properties"].get("custom").is_some());
    assert!(tree["properties"].get("children").is_none());
}
#[test]
fn sse_paths_resolve_before_join_and_event_stream_media_is_retained() {
    let mut s = ProjectSettings::default();
    s.config("application.yml", "sse:\n  path: /resource/sse")
        .unwrap();
    let classes = source::parse(
        "Api.java",
        r#"import org.springframework.web.servlet.mvc.method.annotation.SseEmitter;
 @RestController class Api{ @GetMapping(value="${sse.path}",produces=MediaType.TEXT_EVENT_STREAM_VALUE) public SseEmitter connect(){return null;} @GetMapping("${sse.path}/close") public void close(){} }"#,
    );
    let r = generate_with_project(&classes, &Scope::default(), "测试", &s.finish());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.endpoints.len(), 2);
    assert_eq!(
        r.document["paths"]["/resource/sse"]["get"]["responses"]["200"]["content"]["text/event-stream"]
            ["schema"]["type"],
        "string"
    );
    assert!(r.document["paths"].get("/resource/sse/close").is_some());
    let missing = generate_with_project(
        &classes,
        &Scope::default(),
        "测试",
        &ProjectContext::default(),
    );
    assert_eq!(missing.warnings.len(), 2);
    assert!(missing.endpoints.is_empty());
}
#[test]
fn exclude_test_source_sets_but_keep_production_packages_named_test() {
    for p in [
        "a/src/test/java/Api.java",
        "a/src/test/kotlin/Api.kt",
        "a/src/testFixtures/java/Api.java",
        "a/src/integrationTest/java/Api.java",
        "tests/Api.java",
        r"a\src\test\java\Api.java",
    ] {
        assert!(!source::production_source(p), "{p}");
    }
    assert!(source::production_source(
        "a/src/main/java/demo/test/TestController.java"
    ));
}
#[test]
fn wildcard_external_tree_import_is_not_shadowed_by_an_unrelated_project_class() {
    let mut classes = source::parse(
        "Api.java",
        r#"package api; import cn.hutool.core.lang.tree.*; @RestController class Api{ @GetMapping("/tree") public Tree<Long> tree(){return null;} }"#,
    );
    classes.extend(source::parse(
        "custom/Tree.java",
        "package custom; class Tree<T>{private T custom;}",
    ));
    let r = generate_with_project(
        &classes,
        &Scope::default(),
        "测试",
        &ProjectContext::default(),
    );
    assert!(r.warnings.is_empty());
    assert!(
        r.document["components"]["schemas"]["TreeLong"]["properties"]
            .get("children")
            .is_some()
    );
}
#[test]
fn maven_placeholder_with_suffix_does_not_break_yaml_sibling_properties() {
    let mut s = ProjectSettings::default();
    s.config(
        "application.yml",
        "ruoyi:\n  buildVersion: @currentDate@Z # build time\nserver:\n  port: 9090\nsse:\n  path: /resource/sse\nspring:\n  profiles:\n    active: @profiles.active@\n",
    )
    .unwrap();
    let c = s.finish();
    assert_eq!(c.debug_url(), "http://localhost:9090");
    assert_eq!(c.route("${sse.path}/close").unwrap(), "/resource/sse/close");
    assert!(c.route("${ruoyi.buildVersion}").is_err());
}
