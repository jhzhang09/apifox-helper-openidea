//! 项目切换后，旧面板请求不得读写当前项目或发送网络请求。
use apifox_helper_openidea::plugin::{ApifoxHelper, load_config};
use openidea_plugin::{
    host,
    mock::MockHost,
    views::{ViewEvent, Views},
};
use serde_json::{Value, json};

fn message(h: &MockHost, value: Value) -> Value {
    ApifoxHelper
        .handle(
            "apifox",
            ViewEvent::Message(value.to_string()),
            &h.invocation(),
        )
        .unwrap();
    serde_json::from_str(&h.view_messages().last().unwrap().1).unwrap()
}
#[test]
fn stale_project_actions_are_rejected_before_reads_writes_navigation_or_network() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "storage",
        "secrets",
        "net:api.apifox.com",
    ]);
    h.set_project_root(Some("/B"));
    host::storage::set("config:/B", r#"{"project_id":"222"}"#).unwrap();
    for action in [
        "scan",
        "upload",
        "export",
        "save-config",
        "debug",
        "navigate",
        "list-projects",
    ] {
        let response = message(
            &h,
            json!({"op":action,"projectRoot":"/A","clientId":"panel-A","requestId":37,"config":{"project_id":"111"},"url":"https://api.apifox.com/"}),
        );
        assert_eq!(response["op"], "config", "{action}");
        assert_eq!(response["projectRoot"], "/B");
        assert_eq!(response["config"]["project_id"], "222");
        assert_eq!(response["contextChanged"], true);
        assert_eq!(response["clientId"], "panel-A");
        assert_eq!(response["requestId"], 37);
        assert!(response["message"].as_str().unwrap().contains("已阻止"));
    }
    assert!(h.requests().is_empty());
    assert_eq!(load_config(&h.invocation()).unwrap().project_id, "222");
    assert!(
        !h.invocation()
            .project()
            .unwrap()
            .exists("apifox-openapi.json")
            .unwrap()
    );
    assert!(h.opened_urls().is_empty());
}
#[test]
fn context_probe_and_init_use_invoking_project_and_echo_request_identity() {
    let h = MockHost::install();
    h.set_project_root(Some("/A"));
    host::storage::set("config:/A", r#"{"project_id":"111"}"#).unwrap();
    h.set_project_root(Some("/B"));
    host::storage::set("config:/B", r#"{"project_id":"222"}"#).unwrap();
    let changed = message(
        &h,
        json!({"op":"context","projectRoot":"/A","requestId":2,"clientId":"panel"}),
    );
    assert_eq!(changed["config"]["project_id"], "222");
    assert_eq!(changed["contextChanged"], true);
    let same = message(
        &h,
        json!({"op":"context","projectRoot":"/B","requestId":3,"clientId":"panel"}),
    );
    assert_eq!(
        same,
        json!({"op":"context","projectRoot":"/B","requestId":3,"clientId":"panel"})
    );
    h.set_project_root(Some("/A"));
    let back = message(&h, json!({"op":"init","requestId":4,"clientId":"panel"}));
    assert_eq!(back["config"]["project_id"], "111");
    h.set_project_root(None);
    let closed = message(
        &h,
        json!({"op":"context","projectRoot":"/A","requestId":5,"clientId":"panel"}),
    );
    assert_eq!(closed["projectRoot"], Value::Null);
    assert_eq!(closed["config"]["project_id"], "");
}
#[test]
fn current_scan_and_errors_are_bound_to_project_window_and_request() {
    let h = MockHost::install();
    h.add_file(
        "Api.java",
        r#"@RestController class Api { @GetMapping("/ok") public String get(){return null;} }"#,
    );
    let value = message(
        &h,
        json!({"op":"scan","projectRoot":"/project","clientId":"panel","requestId":7}),
    );
    assert_eq!(value["op"], "scan");
    assert_eq!(value["export"]["endpoints"].as_array().unwrap().len(), 1);
    assert_eq!(value["projectRoot"], "/project");
    assert_eq!(value["requestId"], 7);
    let missing = message(&h, json!({"op":"debug","clientId":"panel","requestId":8}));
    assert_eq!(missing["op"], "config");
    let error = message(
        &h,
        json!({"op":"debug","projectRoot":"/project","clientId":"panel","requestId":9,"url":"file:///tmp/ignored"}),
    );
    assert_eq!(error["op"], "error");
    assert_eq!(error["requestId"], 9);
    assert_eq!(error["projectRoot"], "/project");
    assert!(h.requests().is_empty());
}
