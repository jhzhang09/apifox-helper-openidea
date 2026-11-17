//! 持久化快照只用于恢复预览，上传请求必须使用当时重新扫描得到的契约。
use apifox_helper_openidea::{plugin::ApifoxHelper, snapshot};
use openidea_plugin::{
    Plugin, host,
    mock::MockHost,
    views::{Body, ViewEvent, Views},
};
use serde_json::{Value, json};

fn message(h: &MockHost, op: Value) -> Value {
    let root = h.invocation().project().map(|p| p.root().unwrap());
    let mut request = op;
    request["projectRoot"] = json!(root);
    request["clientId"] = json!("snapshot-test");
    request["requestId"] = json!(1);
    ApifoxHelper
        .handle(
            "apifox",
            ViewEvent::Message(request.to_string()),
            &h.invocation(),
        )
        .unwrap();
    serde_json::from_str(&h.view_messages().last().unwrap().1).unwrap()
}
fn api(path: &str) -> String {
    format!(
        "package demo; @RestController class Api {{ @GetMapping(\"{path}\") public User get(){{return null;}} }}"
    )
}
fn setup() -> MockHost {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "secrets",
        "net:api.apifox.com",
    ]);
    h.add_file("src/Api.java", &api("/user"));
    h.add_file("src/User.java", "package demo; class User { String old; }");
    h
}
#[test]
fn full_scan_persists_time_and_restores_after_project_close_and_new_plugin_instance() {
    let h = setup();
    let scanned = message(&h, json!({"op":"scan"}));
    assert_eq!(scanned["op"], "scan");
    assert_eq!(scanned["snapshotSaved"], true);
    let time = scanned["scannedAtMs"].as_u64().unwrap();
    assert!(time > 1_000_000_000_000);
    h.set_project_root(None);
    assert_eq!(
        message(&h, json!({"op":"init"}))["projectRoot"],
        Value::Null
    );
    h.set_project_root(Some("/project"));
    let restored = message(&h, json!({"op":"init"}));
    assert_eq!(restored["snapshot"]["scannedAtMs"], time);
    assert_eq!(restored["snapshot"]["export"], scanned["export"]);
    assert_eq!(restored["snapshot"]["projectRoot"], "/project");
    let view = ApifoxHelper
        .render("apifox", None, &h.invocation())
        .unwrap();
    assert!(
        matches!(view.body, Body::Html(page) if page.contains("\"snapshotAvailable\":true") && !page.contains("snapshot-test") && !page.contains("\"/user\""))
    );
    assert!(h.requests().is_empty());
}
#[test]
fn snapshots_are_isolated_by_local_root_and_partial_commands_do_not_replace_them() {
    let h = setup();
    let a = message(&h, json!({"op":"scan"}));
    h.set_project_root(Some("/other"));
    assert!(message(&h, json!({"op":"init"}))["snapshot"].is_null());
    h.add_file("src/Api.java", &api("/other"));
    let b = message(&h, json!({"op":"scan"}));
    assert!(b["export"]["document"]["paths"].get("/other").is_some());
    h.set_project_root(Some("/project"));
    let restored = message(&h, json!({"op":"init"}));
    assert_eq!(restored["snapshot"]["export"], a["export"]);
    assert_ne!(restored["snapshot"]["export"], b["export"]);
    assert_eq!(host::storage::keys("scan-snapshot:").unwrap().len(), 2);
    h.add_file("src/Api.java", &api("/partial"));
    h.select_paths(&["src/Api.java"]);
    ApifoxHelper
        .run_command("export-openapi", &h.invocation(), None)
        .unwrap();
    assert_eq!(
        message(&h, json!({"op":"init"}))["snapshot"]["export"],
        a["export"]
    );
}
#[test]
fn failed_scan_keeps_previous_snapshot_and_reports_the_error() {
    let h = setup();
    message(&h, json!({"op":"scan"}));
    let saved = host::storage::get("scan-snapshot:/project")
        .unwrap()
        .unwrap();
    h.add_file("src/Api.java", "class Api {");
    assert_eq!(message(&h, json!({"op":"scan"}))["op"], "error");
    assert_eq!(
        host::storage::get("scan-snapshot:/project")
            .unwrap()
            .unwrap(),
        saved
    );
    assert!(message(&h, json!({"op":"init"}))["snapshot"].is_object());
}
#[test]
fn corrupt_incompatible_or_foreign_snapshots_do_not_block_config_or_reappear_in_another_project() {
    let h = setup();
    message(&h, json!({"op":"scan"}));
    let valid = host::storage::get("scan-snapshot:/project")
        .unwrap()
        .unwrap();
    let mut foreign: Value = serde_json::from_str(&valid).unwrap();
    foreign["projectRoot"] = json!("/different");
    let mut unsupported: Value = serde_json::from_str(&valid).unwrap();
    unsupported["version"] = json!(999);
    for broken in ["{".into(), foreign.to_string(), unsupported.to_string()] {
        host::storage::set("scan-snapshot:/project", &broken).unwrap();
        let result = message(&h, json!({"op":"init"}));
        assert_eq!(result["op"], "config");
        assert!(result["config"].is_object());
        assert!(result["snapshot"].is_null());
        assert!(result["snapshotError"].as_str().unwrap().contains("快照"));
    }
    message(&h, json!({"op":"scan"}));
    assert!(snapshot::load("/project").unwrap().is_some());
}
#[test]
fn oversized_snapshot_rejects_save_without_replacing_the_last_result() {
    let h = setup();
    message(&h, json!({"op":"scan"}));
    let saved = host::storage::get("scan-snapshot:/project")
        .unwrap()
        .unwrap();
    let mut value = snapshot::load("/project").unwrap().unwrap();
    value.export.document["description"] = json!("x".repeat(6 * 1024 * 1024));
    assert!(value.save().unwrap_err().to_string().contains("6 MiB"));
    assert_eq!(
        host::storage::get("scan-snapshot:/project")
            .unwrap()
            .unwrap(),
        saved
    );
}
#[test]
fn upload_after_restore_rescans_and_uses_new_fields_without_trusting_cached_or_client_documents() {
    let h = setup();
    let scanned = message(&h, json!({"op":"scan"}));
    let id = scanned["export"]["endpoints"][0]["operation"]["operationId"].clone();
    message(&h, json!({"op":"init"}));
    h.add_file(
        "src/User.java",
        "package demo; class User { String fresh; }",
    );
    host::storage::set("config:/project", r#"{"project_id":"123"}"#).unwrap();
    h.set_secret("apifox_token", "synthetic-test-token");
    h.answer_next_prompt(Some(&["yes"]));
    h.on_fetch(|request| {
        let payload: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
        let document: Value = serde_json::from_str(payload["input"].as_str().unwrap()).unwrap();
        assert_eq!(
            document["components"]["schemas"]["User"]["properties"]["fresh"]["type"],
            "string"
        );
        assert!(
            document["components"]["schemas"]["User"]["properties"]
                .get("old")
                .is_none()
        );
        Ok(openidea_plugin::HttpResponse {
            status: 200,
            body: br#"{"data":{"counters":{"endpointCreated":1,"endpointUpdated":0}}}"#.to_vec(),
            ..Default::default()
        })
    });
    let uploaded = message(
        &h,
        json!({"op":"upload","selected":[id],"document":scanned["export"]["document"]}),
    );
    assert_eq!(uploaded["op"], "notice", "{uploaded}");
    assert_eq!(h.requests().len(), 1);
}
#[test]
fn upload_of_disappeared_or_now_invalid_endpoint_is_blocked_after_restore() {
    let h = setup();
    let scanned = message(&h, json!({"op":"scan"}));
    let id = scanned["export"]["endpoints"][0]["operation"]["operationId"].clone();
    for source in [api("/changed"), "class Api {".into()] {
        h.add_file("src/Api.java", &source);
        let result = message(&h, json!({"op":"upload","selected":[id]}));
        assert_eq!(result["op"], "error", "{result}");
    }
    assert!(h.requests().is_empty());
}
