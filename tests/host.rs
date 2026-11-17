use apifox_helper_openidea::{
    apifox::{self, Config},
    plugin::{ApifoxHelper, debug_request, filter_export, scan},
};
use openidea_plugin::views::{Body, ViewEvent, Views};
use openidea_plugin::{Error, Plugin, host, mock::MockHost};
use serde_json::{Value, json};
const API: &str = r#"package demo; @RestController class Api { @GetMapping("/first") public User first(){return null;} @GetMapping("/second") public User second(){return null;} } class User { private Address address; } class Address { private String city; }"#;
#[test]
fn scans_project_selected_directory_and_unsaved_buffers() {
    let h = MockHost::install();
    h.add_file("src/Api.java", API);
    h.select_paths(&["/project/src"]);
    let export = scan(&h.invocation(), "selection").unwrap();
    assert_eq!(export.endpoints.len(), 2);
    h.open_editor(
        "/project/src/Api.java",
        "java",
        &API.replace("/first", "/edited"),
        &[(0, 0, 0, 0)],
    );
    let export = scan(&h.invocation(), "project").unwrap();
    assert!(export.document["paths"].get("/edited").is_some());
    assert!(export.document["paths"].get("/first").is_none());
}
#[test]
fn denied_project_access_does_not_produce_success() {
    let h = MockHost::install().with_capabilities(&[]);
    h.add_file("Api.java", API);
    assert!(matches!(
        scan(&h.invocation(), "project"),
        Err(Error::Denied(_))
    ));
}
#[test]
fn stable_ids_filter_selection_and_reject_disappeared_endpoints() {
    let h = MockHost::install();
    h.add_file("Api.java", API);
    let all = scan(&h.invocation(), "project").unwrap();
    let id = all.endpoints[1].operation["operationId"]
        .as_str()
        .unwrap()
        .to_string();
    let part = filter_export(all.clone(), &[id]).unwrap();
    assert_eq!(part.endpoints.len(), 1);
    assert_eq!(part.document["paths"].as_object().unwrap().len(), 1);
    assert!(filter_export(all, &["stale-id".into()]).is_err());
}
#[test]
fn import_payload_uses_official_contract_and_secret_placeholder() {
    let c = Config {
        project_id: "12345".into(),
        endpoint_folder: Some(76),
        schema_folder: Some(60),
        ..Default::default()
    };
    let request = apifox::import_request(&c, &json!({"openapi":"3.0.3"})).unwrap();
    assert_eq!(request.method, "POST");
    assert!(
        request
            .url
            .starts_with("https://api.apifox.com/v1/projects/12345/import-openapi")
    );
    assert!(request.headers.contains(&(
        "Authorization".into(),
        "Bearer {{secret:apifox_token}}".into()
    )));
    let v: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
    assert!(v["input"].is_string());
    assert_eq!(v["options"]["deleteUnmatchedResources"], false);
    assert_eq!(v["options"]["targetSchemaFolderId"], 60);
    assert_eq!(v["options"]["targetEndpointFolderId"], 76);
}
#[test]
fn import_errors_and_partial_failures_never_report_success() {
    for (status, body) in [
        (401, "{}"),
        (200, "{}"),
        (
            200,
            r#"{"data":{"counters":{"endpointCreated":1,"endpointUpdated":0,"schemaFailed":1}}}"#,
        ),
        (
            200,
            r#"{"data":{"counters":{"endpointCreated":1,"endpointUpdated":0},"errors":[{"message":"bad schema"}]}}"#,
        ),
    ] {
        assert!(apifox::outcome(status, body).is_err(), "{body}");
    }
    assert!(
        apifox::outcome(
            200,
            r#"{"data":{"counters":{"endpointCreated":1,"endpointUpdated":0}}}"#
        )
        .unwrap()
        .contains("新增 1")
    );
}
#[test]
fn debug_rejects_other_protocols_header_injection_and_sync_secret() {
    for v in [
        json!({"url":"file:///etc/passwd"}),
        json!({"url":"http://user:password@localhost/"}),
        json!({"url":"http://localhost/","headers":[["x","a\r\nb"]]}),
        json!({"url":"https://example.com/","headers":[["Authorization","Bearer {{secret:apifox_token}}"]]}),
    ] {
        assert!(debug_request(&v).is_err());
    }
    assert_eq!(
        debug_request(&json!({"url":"http://localhost:8080/api","method":"POST","body":"{}"}))
            .unwrap()
            .timeout_ms,
        Some(30000)
    );
}
#[test]
fn renders_panel_and_routes_errors_to_the_page() {
    let h = MockHost::install();
    let mut plugin = ApifoxHelper;
    let view = plugin.render("apifox", None, &h.invocation()).unwrap();
    assert!(matches!(view.body,Body::Html(page) if page.contains("响应模型")));
    plugin
        .handle(
            "apifox",
            ViewEvent::Message(json!({"op":"init"}).to_string()),
            &h.invocation(),
        )
        .unwrap();
    let messages = h.view_messages();
    let v: Value = serde_json::from_str(&messages.last().unwrap().1).unwrap();
    assert_eq!(v["op"], "config");
    plugin
        .handle(
            "apifox",
            ViewEvent::Message(json!({"op":"scan","scope":"project"}).to_string()),
            &h.invocation(),
        )
        .unwrap();
    let v: Value = serde_json::from_str(&h.view_messages().last().unwrap().1).unwrap();
    assert_eq!(v["op"], "error");
    assert!(
        plugin
            .run_command("not-found", &h.invocation(), None)
            .is_err()
    );
}
#[test]
fn manifest_has_matching_commands_views_and_secret() {
    let manifest: toml::Value = toml::from_str(include_str!("../plugin.toml")).unwrap();
    assert_eq!(manifest["id"].as_str(), Some("jhzhang.apifox-helper"));
    assert_eq!(
        manifest["contributes"]["commands"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    assert_eq!(
        manifest["contributes"]["views"][0]["id"].as_str(),
        Some("apifox")
    );
    assert_eq!(
        manifest["contributes"]["secrets"][0]["name"].as_str(),
        Some("apifox_token")
    );
    let _ = host::storage::keys;
}
#[test]
fn upload_requires_confirmation_and_host_replaces_secret_without_uploading_source() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "secrets",
        "net:api.apifox.com",
    ]);
    h.add_file("Api.java", API);
    h.select_paths(&["Api.java"]);
    h.set_secret("apifox_token", "test-token-only");
    host::storage::set(
        "config:/project",
        &serde_json::to_string(&Config {
            project_id: "123".into(),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    h.on_fetch(|req|{assert!(req.headers.contains(&("Authorization".into(),"Bearer test-token-only".into())));let v:Value=serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();let d:Value=serde_json::from_str(v["input"].as_str().unwrap()).unwrap();assert!(d["components"]["schemas"].is_object());assert!(!v["input"].as_str().unwrap().contains("return null"));Ok(openidea_plugin::HttpResponse{status:200,body:br#"{"data":{"counters":{"endpointCreated":2,"endpointUpdated":0,"schemaCreated":2}}}"#.to_vec(),..Default::default()})});
    let mut plugin = ApifoxHelper;
    assert!(
        plugin
            .run_command("upload", &h.invocation(), None)
            .unwrap()
            .is_none()
    );
    assert!(h.requests().is_empty());
    h.answer_next_prompt(Some(&["yes"]));
    assert!(
        plugin
            .run_command("upload", &h.invocation(), None)
            .unwrap()
            .unwrap()
            .contains("新增 2")
    );
    assert_eq!(h.requests().len(), 1);
}
#[test]
fn malformed_sources_and_unsupported_legacy_rules_are_visible() {
    let h = MockHost::install();
    h.add_file("Api.java", "class Api {");
    assert!(
        scan(&h.invocation(), "project")
            .unwrap_err()
            .to_string()
            .contains("未闭合")
    );
    h.add_file("Api.java", API);
    h.add_file(".apifox-helper.properties","param.ignore=@org.springframework.web.bind.annotation.RequestAttribute\nfield.doc=groovy:someScript\n");
    let r = scan(&h.invocation(), "project").unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("field.doc")));
}
#[test]
fn selected_endpoints_only_include_reachable_models() {
    let h = MockHost::install();
    h.add_file("Api.java",r#"@RestController class Api { @GetMapping("/u") public User user(){return null;} @GetMapping("/o") public Other other(){return null;} } class User { private Address address; } class Address { private User owner; } class Other { private int count; }"#);
    let all = scan(&h.invocation(), "project").unwrap();
    let id = all
        .endpoints
        .iter()
        .find(|e| e.path == "/u")
        .unwrap()
        .operation["operationId"]
        .as_str()
        .unwrap()
        .to_string();
    let part = filter_export(all, &[id]).unwrap();
    let schemas = part.document["components"]["schemas"].as_object().unwrap();
    assert_eq!(schemas.len(), 2);
    assert!(schemas.contains_key("User"));
    assert!(schemas.contains_key("Address"));
    assert!(!schemas.contains_key("Other"));
}

#[test]
fn structure_notices_do_not_block_explicitly_confirmed_sync() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "secrets",
        "net:api.apifox.com",
    ]);
    h.add_file("Api.java",r#"@RestController class Api{ @GetMapping("/raw") public R raw(){return null;} } class R<T>{private T data;}"#);
    h.select_paths(&["Api.java"]);
    h.set_secret("apifox_token", "test-only");
    host::storage::set(
        "config:/project",
        &serde_json::to_string(&Config {
            project_id: "123".into(),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    let export = scan(&h.invocation(), "selection").unwrap();
    assert!(export.warnings.is_empty());
    assert_eq!(export.notices.len(), 1);
    h.on_fetch(|_| {
        Ok(openidea_plugin::HttpResponse {
            status: 200,
            body: br#"{"data":{"counters":{"endpointCreated":1,"endpointUpdated":0}}}"#.to_vec(),
            ..Default::default()
        })
    });
    h.answer_next_prompt(Some(&["yes"]));
    assert!(
        ApifoxHelper
            .run_command("upload", &h.invocation(), None)
            .unwrap()
            .unwrap()
            .contains("新增 1")
    );
    assert_eq!(h.requests().len(), 1);
}

#[test]
fn navigation_metadata_uses_source_package_controller_and_maven_artifact() {
    let h = MockHost::install();
    h.add_file("sample-web/pom.xml",r#"<project><parent><artifactId>parent-name</artifactId></parent><!-- <artifactId>comment</artifactId> --><artifactId>sample-server</artifactId><dependencies><dependency><artifactId>dependency-name</artifactId></dependency></dependencies></project>"#);
    h.add_file("sample-web/src/main/java/demo/AdminController.java",r#"package demo.web; @Tag(name="用户管理") @RestController class AdminController { @GetMapping("/users") public void users(){} }"#);
    let export = scan(&h.invocation(), "project").unwrap();
    let e = &export.endpoints[0];
    assert_eq!(e.module, "sample-server");
    assert_eq!(e.module_path, "sample-web");
    assert_eq!(e.package, "demo.web");
    assert_eq!(e.controller, "AdminController");
    assert_eq!(e.controller_name, "用户管理");
    assert!(
        export.document["paths"]["/users"]["get"]
            .get("module")
            .is_none()
    );
}

#[test]
fn maven_module_name_falls_back_for_missing_or_unresolved_artifact() {
    use apifox_helper_openidea::navigation::pom_artifact_id;
    assert_eq!(
        pom_artifact_id("<project><parent><artifactId>parent</artifactId></parent></project>"),
        None
    );
    assert_eq!(
        pom_artifact_id("<project><artifactId>${custom.name}</artifactId></project>"),
        None
    );
    assert_eq!(
        pom_artifact_id(
            "<?xml version=\"1.0\"?><m:project><m:artifactId>module-a</m:artifactId></m:project>"
        ),
        Some("module-a".into())
    );
    let h = MockHost::install();
    h.add_file("services/web/src/main/java/Api.java", API);
    let export = scan(&h.invocation(), "project").unwrap();
    assert_eq!(export.endpoints[0].module, "web");
}

fn view_message(plugin: &mut ApifoxHelper, h: &MockHost, value: Value) -> Value {
    plugin
        .handle(
            "apifox",
            ViewEvent::Message(value.to_string()),
            &h.invocation(),
        )
        .unwrap();
    serde_json::from_str(&h.view_messages().last().unwrap().1).unwrap()
}
#[test]
fn context_exports_only_requested_interfaces_and_preserves_format_and_overwrite_checks() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
    ]);
    h.add_file("Api.java", API);
    let export = scan(&h.invocation(), "project").unwrap();
    let id = export.endpoints[1].operation["operationId"].clone();
    let mut p = ApifoxHelper;
    let request = json!({"op":"export","scope":"project","selected":[id],"format":"yaml"});
    assert_eq!(view_message(&mut p, &h, request.clone())["op"], "notice");
    let project = h.invocation().project().unwrap();
    let yaml = project.read_file("apifox-openapi.yaml").unwrap();
    assert!(yaml.starts_with("\"openapi\": \"3.0.3\""));
    assert!(yaml.contains("\"/second\":"));
    assert!(!yaml.contains("\"/first\":"));
    assert_eq!(view_message(&mut p, &h, request.clone())["op"], "error");
    assert_eq!(project.read_file("apifox-openapi.yaml").unwrap(), yaml);
    h.answer_next_prompt(Some(&["yes"]));
    assert_eq!(view_message(&mut p, &h, request)["op"], "notice");
    assert_eq!(
        view_message(
            &mut p,
            &h,
            json!({"op":"export","scope":"project","selected":[id],"format":"json"})
        )["op"],
        "notice"
    );
    let json: Value =
        serde_json::from_str(&project.read_file("apifox-openapi.json").unwrap()).unwrap();
    assert_eq!(json["paths"].as_object().unwrap().len(), 1);
    assert!(json["paths"].get("/second").is_some());
    assert_eq!(
        view_message(
            &mut p,
            &h,
            json!({"op":"export","scope":"project","selected":[id],"format":"xml"})
        )["op"],
        "error"
    );
}
#[test]
fn navigation_resolves_fresh_source_lines_and_rejects_unknown_interface_ids() {
    let h = MockHost::install();
    h.add_file("Api.java", API);
    let e = scan(&h.invocation(), "project").unwrap();
    let id = e.endpoints[0].operation["operationId"].clone();
    h.add_file("Api.java", &format!("\n\n\n{API}"));
    let mut p = ApifoxHelper;
    assert_eq!(
        view_message(&mut p, &h, json!({"op":"navigate","operationId":id}))["op"],
        "notice"
    );
    assert_eq!(
        h.opened_locations(),
        vec![("/project/Api.java".into(), 3, 0)]
    );
    assert_eq!(
        view_message(&mut p, &h, json!({"op":"navigate","operationId":"invalid"}))["op"],
        "error"
    );
    assert_eq!(h.opened_locations().len(), 1);
}
#[test]
fn project_scan_excludes_tests_before_parsing_and_type_lookup() {
    let h = MockHost::install();
    h.add_file("app/src/main/java/Api.java", API);
    h.add_file("app/src/test/java/Broken.java", "class Broken {");
    h.add_file(
        "app/src/test/java/User.java",
        "class User {private Missing onlyInTest;}",
    );
    h.add_file("app/src/test/resources/application.yml", "not: [valid");
    let r = scan(&h.invocation(), "project").unwrap();
    assert!(r.warnings.is_empty());
    assert_eq!(r.endpoints.len(), 2);
    assert!(!r.document.to_string().contains("onlyInTest"));
}
#[test]
fn default_panel_scan_is_project_wide_and_uses_application_config() {
    let h = MockHost::install();
    h.add_file("app/src/main/java/Api.java", API);
    h.add_file(
        "app/src/main/java/Sse.java",
        r#"@RestController class Sse{ @GetMapping("${sse.path}/close") public void close(){} }"#,
    );
    h.add_file(
        "app/src/main/resources/application.yml",
        "sse:\n  path: /resource/sse",
    );
    h.select_paths(&["app/src/main/java/Api.java"]);
    let r = view_message(&mut ApifoxHelper, &h, json!({"op":"scan"}));
    assert_eq!(r["op"], "scan");
    assert_eq!(r["export"]["endpoints"].as_array().unwrap().len(), 3);
    assert_eq!(r["export"]["warnings"], json!([]));
    assert!(
        r["export"]["document"]["paths"]
            .get("/resource/sse/close")
            .is_some()
    );
}
#[test]
fn project_config_survives_new_plugin_instances_and_is_scoped_by_local_root() {
    let h = MockHost::install();
    let mut plugin = ApifoxHelper;
    let config = json!({"project_id":"12345","endpoint_folder":76,"schema_folder":60,"title":"测试项目","base_url":"http://localhost:9000"});
    let saved = view_message(&mut plugin, &h, json!({"op":"save-config","config":config}));
    assert_eq!(saved["op"], "config");
    assert_eq!(saved["message"], "配置已保存");
    assert_eq!(saved["projectRoot"], "/project");
    assert_eq!(saved["config"]["project_id"], "12345");
    let mut reopened = ApifoxHelper;
    let state = view_message(&mut reopened, &h, json!({"op":"init"}));
    assert_eq!(state["config"]["project_id"], "12345");
    assert_eq!(state["config"]["endpoint_folder"], 76);
    h.set_project_root(Some("/another-project"));
    assert_eq!(
        view_message(&mut reopened, &h, json!({"op":"init"}))["config"]["project_id"],
        ""
    );
    h.set_project_root(Some("/project"));
    assert_eq!(
        view_message(&mut reopened, &h, json!({"op":"init"}))["config"]["project_id"],
        "12345"
    );
}
#[test]
fn render_embeds_saved_config_without_exposing_secret_or_allowing_script_injection() {
    let h = MockHost::install().with_capabilities(&["project.read", "storage", "secrets"]);
    h.set_secret("apifox_token", "secret-test-token");
    let config = Config {
        project_id: "12345".into(),
        title: "</script><script>injected()</script>\u{2028}".into(),
        ..Default::default()
    };
    host::storage::set("config:/project", &serde_json::to_string(&config).unwrap()).unwrap();
    let view = ApifoxHelper
        .render("apifox", None, &h.invocation())
        .unwrap();
    let Body::Html(page) = view.body else {
        panic!("HTML 侧栏")
    };
    assert!(page.contains(r#""project_id":"12345""#));
    assert!(page.contains(r#""projectRoot":"/project""#));
    assert!(!page.contains("__APIFOX_CONFIG__"));
    assert!(!page.contains("secret-test-token"));
    assert!(!page.contains("</script><script>injected()"));
    assert!(page.contains("\\u003c/script>"));
}
#[test]
fn invalid_config_save_does_not_replace_existing_storage() {
    let h = MockHost::install();
    host::storage::set(
        "config:/project",
        &serde_json::to_string(&Config {
            project_id: "12345".into(),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    let response = view_message(
        &mut ApifoxHelper,
        &h,
        json!({"op":"save-config","config":{"project_id":"invalid"}}),
    );
    assert_eq!(response["op"], "error");
    assert_eq!(
        apifox_helper_openidea::plugin::load_config(&h.invocation())
            .unwrap()
            .project_id,
        "12345"
    );
}
