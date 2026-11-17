//! 官方插件兼容查询协议、错误处理、令牌隔离和持久化的回归。
use apifox_helper_openidea::{
    apifox::{self, Config},
    plugin::{ApifoxHelper, load_config},
};
use openidea_plugin::views::{ViewEvent, Views};
use openidea_plugin::{Error, HttpResponse, host, mock::MockHost};
use serde_json::{Value, json};
fn mock() -> MockHost {
    MockHost::install().with_capabilities(&[
        "storage",
        "project.read",
        "secrets",
        "net:api.apifox.com",
    ])
}
#[test]
fn project_requests_match_official_plugin_protocol_and_never_put_token_in_url() {
    for teams in [false, true] {
        let r = apifox::project_list_request(teams);
        assert_eq!(r.method, "GET");
        assert!(r.body.is_none());
        assert!(r.url.starts_with("https://api.apifox.com/api/v1/user-"));
        assert!(!r.url.contains("secret"));
        assert!(
            r.headers
                .contains(&("X-Apifox-Version".into(), "2024-03-28".into()))
        );
        assert!(r.headers.contains(&(
            "Authorization".into(),
            "Bearer {{secret:apifox_token}}".into()
        )));
        assert_eq!(r.timeout_ms, Some(30000));
    }
}
#[test]
fn project_parser_handles_numeric_string_ids_deduplicates_and_filters_non_http() {
    let rows = json!({"data":[{"id":12,"name":"同名项目","teamId":1,"type":"HTTP"},{"id":"12","name":"同名项目","teamId":"1","type":"HTTP"},{"id":"9007199254740993","name":"同名项目","teamId":"2","type":"HTTP"},{"id":44,"name":"RPC","teamId":1,"type":"GRPC"}]});
    let projects = apifox::parse_projects(200, &rows.to_string()).unwrap();
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[1].id, "9007199254740993");
    assert_ne!(projects[0].team_id, projects[1].team_id);
}
#[test]
fn failures_malformed_envelopes_and_conflicting_projects_are_not_empty_success() {
    for (status, body) in [
        (401, "{}"),
        (403, "{}"),
        (429, "{}"),
        (500, "{}"),
        (200, "not-json"),
        (200, "{}"),
        (200, r#"{"success":false,"data":[]}"#),
        (200, r#"{"errorCode":"500001","data":[]}"#),
        (
            200,
            r#"{"data":[{"id":1,"name":"missing type","teamId":1}]}"#,
        ),
        (
            200,
            r#"{"data":[{"id":"bad-id","name":"bad","teamId":1,"type":"HTTP"}]}"#,
        ),
        (
            200,
            r#"{"data":[{"id":1,"name":"a","teamId":1,"type":"HTTP"},{"id":1,"name":"b","teamId":1,"type":"HTTP"}]}"#,
        ),
    ] {
        assert!(
            apifox::parse_projects(status, body).is_err(),
            "{status} {body}"
        );
    }
    assert!(
        apifox::parse_projects(200, r#"{"data":[]}"#)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn host_fetches_projects_and_teams_with_keychain_substitution_only() {
    let h = mock();
    h.set_secret("apifox_token", "mock-secret-only");
    h.on_fetch(|r| {
        assert_eq!(r.method, "GET");
        assert!(
            r.headers
                .contains(&("Authorization".into(), "Bearer mock-secret-only".into()))
        );
        let body = if r.url.ends_with("/user-projects") {
            json!({"data":[{"id":1,"name":"测试","teamId":8,"type":"HTTP"}]})
        } else {
            json!({"data":[{"id":8,"name":"团队 A"}]})
        };
        Ok(HttpResponse {
            status: 200,
            body: body.to_string().into_bytes(),
            ..Default::default()
        })
    });
    let response = apifox::list_projects().unwrap();
    assert_eq!(h.requests().len(), 2);
    assert_eq!(response.projects[0].team_name, "团队 A");
    assert!(response.notices.is_empty());
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("mock-secret-only")
    );
    assert!(host::storage::keys("").unwrap().is_empty());
}
#[test]
fn missing_secret_denied_access_and_server_errors_do_not_mutate_config() {
    let h = mock();
    host::storage::set(
        "config:/project",
        &serde_json::to_string(&Config {
            project_id: "123".into(),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    assert!(apifox::list_projects().is_err());
    assert!(h.requests().is_empty());
    h.set_secret("apifox_token", "test-only");
    h.on_fetch(|_| {
        Ok(HttpResponse {
            status: 401,
            ..Default::default()
        })
    });
    assert!(
        apifox::list_projects()
            .unwrap_err()
            .to_string()
            .contains("过期")
    );
    assert_eq!(h.requests().len(), 1);
    assert_eq!(load_config(&h.invocation()).unwrap().project_id, "123");
    let h = MockHost::install().with_capabilities(&["secrets"]);
    h.set_secret("apifox_token", "test-only");
    assert!(matches!(apifox::list_projects(), Err(Error::Denied(_))));
}
#[test]
fn team_name_failure_preserves_usable_projects_with_visible_notice() {
    let h = mock();
    h.set_secret("apifox_token", "test-only");
    h.on_fetch(|r| {
        if r.url.ends_with("/user-projects") {
            Ok(HttpResponse {
                status: 200,
                body: br#"{"data":[{"id":1,"name":"test","teamId":8,"type":"HTTP"}]}"#.to_vec(),
                ..Default::default()
            })
        } else {
            Err(Error::Failed("temporary failure".into()))
        }
    });
    let r = apifox::list_projects().unwrap();
    assert_eq!(r.projects[0].team_name, "团队 8");
    assert_eq!(r.notices.len(), 1);
}
#[test]
fn view_returns_picker_data_without_saving_and_project_names_roundtrip() {
    let h = mock();
    h.set_secret("apifox_token", "test-only");
    h.on_fetch(|r| {
        Ok(HttpResponse {
            status: 200,
            body: if r.url.ends_with("/user-projects") {
                br#"{"data":[]}"#.to_vec()
            } else {
                panic!("empty projects need no teams request")
            },
            ..Default::default()
        })
    });
    let mut p = ApifoxHelper;
    p.handle(
        "apifox",
        ViewEvent::Message(json!({"op":"list-projects","projectRoot":"/project"}).to_string()),
        &h.invocation(),
    )
    .unwrap();
    let state: Value = serde_json::from_str(&h.view_messages().last().unwrap().1).unwrap();
    assert_eq!(state["op"], "projects");
    assert_eq!(state["projects"], json!([]));
    assert!(host::storage::keys("").unwrap().is_empty());
    p.handle("apifox",ViewEvent::Message(json!({"op":"save-config","projectRoot":"/project","config":{"project_id":"123","project_name":"项目 A","team_name":"团队 A"}}).to_string()),&h.invocation()).unwrap();
    assert_eq!(load_config(&h.invocation()).unwrap().project_name, "项目 A");
    let legacy: Config = serde_json::from_str(r#"{"project_id":"123"}"#).unwrap();
    assert!(legacy.project_name.is_empty());
}
