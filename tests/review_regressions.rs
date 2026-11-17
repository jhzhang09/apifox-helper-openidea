//! 社区审核发现的名称绑定、路径规范化与项目切换问题，均走宿主生产入口。
use apifox_helper_openidea::{
    apifox::Config,
    plugin::{ApifoxHelper, load_config, scan},
};
use openidea_plugin::{Plugin, host, mock::MockHost};
use serde_json::Value;

fn response(export: &Value, path: &str) -> Value {
    let schema =
        &export["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
    let reference = schema["$ref"].as_str().unwrap();
    export
        .pointer(reference.strip_prefix('#').unwrap())
        .unwrap()
        .clone()
}

#[test]
fn explicit_import_wins_over_wildcards_in_both_orders() {
    for imports in [
        "import other.*; import correct.User;",
        "import correct.User; import other.*;",
    ] {
        let h = MockHost::install();
        h.add_file("Api.java", &format!("package api; {imports} @RestController class Api {{ @GetMapping(\"/user\") public User get(){{return null;}} }}"));
        h.add_file(
            "correct/User.java",
            "package correct; public class User { public String correct; }",
        );
        h.add_file(
            "other/User.java",
            "package other; public class User { public String wrong; }",
        );
        let export = scan(&h.invocation(), "project").unwrap();
        assert!(export.warnings.is_empty(), "{:?}", export.warnings);
        let model = response(&export.document, "/user");
        assert_eq!(model["properties"]["correct"]["type"], "string");
        assert!(model["properties"].get("wrong").is_none());
    }
}

#[test]
fn same_package_wins_over_wildcard_and_default_package() {
    let h = MockHost::install();
    h.add_file("own/Api.java", "package own; import other.*; @RestController class Api { @GetMapping(\"/user\") public User get(){return null;} }");
    h.add_file(
        "own/User.java",
        "package own; class User { String correct; }",
    );
    h.add_file(
        "other/User.java",
        "package other; class User { String wrong; }",
    );
    h.add_file("User.java", "class User { String defaultWrong; }");
    let export = scan(&h.invocation(), "project").unwrap();
    assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    let model = response(&export.document, "/user");
    assert_eq!(model["properties"]["correct"]["type"], "string");
    assert_eq!(model["properties"].as_object().unwrap().len(), 1);
}

#[test]
fn conflicting_wildcards_warn_instead_of_picking_the_first_dto() {
    for imports in ["import one.*; import two.*;", "import two.*; import one.*;"] {
        let h = MockHost::install();
        h.add_file("Api.java", &format!("package api; {imports} @RestController class Api {{ @GetMapping(\"/user\") public User get(){{return null;}} }}"));
        h.add_file("one/User.java", "package one; class User { String one; }");
        h.add_file("two/User.java", "package two; class User { String two; }");
        let export = scan(&h.invocation(), "project").unwrap();
        assert!(
            export
                .warnings
                .iter()
                .any(|w| w.contains("歧义") && w.contains("one.User") && w.contains("two.User")),
            "{:?}",
            export.warnings
        );
        assert!(
            export.document["components"]["schemas"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        h.select_paths(&["Api.java"]);
        assert!(
            ApifoxHelper
                .run_command("upload", &h.invocation(), None)
                .is_err()
        );
        assert!(h.requests().is_empty());
    }
}

#[test]
fn unresolved_explicit_import_does_not_fall_back_to_a_wildcard() {
    let h = MockHost::install();
    h.add_file("Api.java", "package api; import other.*; import missing.User; @RestController class Api { @GetMapping(\"/user\") public User get(){return null;} }");
    h.add_file(
        "other/User.java",
        "package other; class User { String wrong; }",
    );
    let export = scan(&h.invocation(), "project").unwrap();
    assert!(!export.warnings.is_empty());
    assert!(
        export.document["components"]["schemas"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn explicit_import_also_wins_over_a_default_package_dto() {
    let h = MockHost::install();
    h.add_file("Api.java", "import correct.User; @RestController class Api { @GetMapping(\"/user\") public User get(){return null;} }");
    h.add_file("User.java", "class User { String wrong; }");
    h.add_file(
        "correct/User.java",
        "package correct; class User { String correct; }",
    );
    let export = scan(&h.invocation(), "project").unwrap();
    assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    assert_eq!(
        response(&export.document, "/user")["properties"]["correct"]["type"],
        "string"
    );
}

const COLLISION: &str = r#"@RestController class Api {
 @GetMapping("/users/{id:[0-9]+}") public String digits(@PathVariable("id") String id){return null;}
 @GetMapping("/users/{id:[a-z]+}") public String letters(@PathVariable("id") String id){return null;}
}"#;

#[test]
fn normalized_route_collision_is_visible_in_scan_and_export() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
    ]);
    h.add_file("Api.java", COLLISION);
    let export = scan(&h.invocation(), "project").unwrap();
    assert!(
        export
            .warnings
            .iter()
            .any(|w| w.contains("重复路由") && w.contains("/users/{id}"))
    );
    assert_eq!(export.endpoints.len(), 1);
    assert_eq!(export.endpoints[0].handler, "Api.digits");
    assert_eq!(
        export.document["paths"]["/users/{id}"]["get"]["summary"],
        "digits"
    );
    let message = ApifoxHelper
        .run_command("export-project", &h.invocation(), None)
        .unwrap()
        .unwrap();
    assert!(message.contains("1 条错误"), "{message}");
    let written: Value = serde_json::from_str(
        &h.invocation()
            .project()
            .unwrap()
            .read_file("apifox-openapi.json")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(written["paths"], export.document["paths"]);
}

#[test]
fn normalized_route_collision_blocks_the_actual_upload_command() {
    let h = MockHost::install();
    h.add_file("Api.java", COLLISION);
    h.select_paths(&["Api.java"]);
    host::storage::set(
        "config:/project",
        &serde_json::to_string(&Config {
            project_id: "123".into(),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    h.set_secret("apifox_token", "synthetic-token");
    h.answer_next_prompt(Some(&["yes"]));
    let error = ApifoxHelper
        .run_command("upload", &h.invocation(), None)
        .unwrap_err();
    assert!(error.to_string().contains("已阻止上传"));
    assert!(h.requests().is_empty());
}

#[test]
fn route_regex_quantifiers_classes_and_escapes_keep_only_the_variable_name() {
    for regex in [
        r"[0-9]{1,10}",
        r"[0-9]+",
        r"[{}]{1,2}",
        r"\\{[0-9]{1,2}\\}",
        r"[a-z&&[^x]]{2}",
    ] {
        let h = MockHost::install();
        h.add_file("Api.java", &format!(r#"@RestController class Api {{ @GetMapping("/users/{{id:{regex}}}/{{tail}}") public String get(@PathVariable("id") String id, @PathVariable("tail") String tail){{return null;}} }}"#));
        let export = scan(&h.invocation(), "project").unwrap();
        assert!(export.warnings.is_empty(), "{regex}: {:?}", export.warnings);
        assert_eq!(export.endpoints[0].path, "/users/{id}/{tail}", "{regex}");
        let names: Vec<_> = export.document["paths"]["/users/{id}/{tail}"]["get"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["id", "tail"]);
    }
}

#[test]
fn normalized_routes_keep_distinct_http_methods() {
    let h = MockHost::install();
    h.add_file("Api.java", r#"@RestController class Api {
        @GetMapping("/users/{id:[0-9]{1,10}}") public String get(@PathVariable("id") String id){return null;}
        @PostMapping("/users/{id:[a-z]{2}}") public String post(@PathVariable("id") String id){return null;}
    }"#);
    let export = scan(&h.invocation(), "project").unwrap();
    assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    assert_eq!(export.endpoints.len(), 2);
    assert!(export.document["paths"]["/users/{id}"]["get"].is_object());
    assert!(export.document["paths"]["/users/{id}"]["post"].is_object());
}

#[test]
fn malformed_regex_route_is_reported_without_exporting_a_broken_path() {
    let h = MockHost::install();
    h.add_file("Api.java", r#"@RestController class Api {
        @GetMapping("/users/{id:[0-9]{1,10}") public String broken(@PathVariable("id") String id){return null;}
        @GetMapping("/ok") public String ok(){return null;}
    }"#);
    let export = scan(&h.invocation(), "project").unwrap();
    assert_eq!(export.endpoints.len(), 1);
    assert_eq!(export.endpoints[0].path, "/ok");
    assert!(
        export
            .warnings
            .iter()
            .any(|w| w.contains("路由") && w.contains("未闭合"))
    );
}

fn stored_config() -> Config {
    Config {
        project_id: "111".into(),
        project_name: "旧项目".into(),
        team_name: "旧团队".into(),
        endpoint_folder: Some(11),
        schema_folder: Some(22),
        branch: Some(33),
        module: Some(44),
        ..Default::default()
    }
}

#[test]
fn configure_command_clears_old_project_associations_and_uploads_the_new_target() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "editor.read",
        "storage",
        "secrets",
        "net:api.apifox.com",
    ]);
    h.add_file(
        "Api.java",
        "@RestController class Api { @GetMapping(\"/ok\") public String get(){return null;} }",
    );
    h.select_paths(&["Api.java"]);
    host::storage::set(
        "config:/project",
        &serde_json::to_string(&stored_config()).unwrap(),
    )
    .unwrap();
    h.answer_next_prompt(Some(&["222"]));
    h.answer_next_prompt(Some(&[""]));
    ApifoxHelper
        .run_command("configure", &h.invocation(), None)
        .unwrap();
    let config = load_config(&h.invocation()).unwrap();
    assert_eq!(config.project_id, "222");
    assert_eq!(
        (
            config.endpoint_folder,
            config.schema_folder,
            config.branch,
            config.module
        ),
        (None, None, None, None)
    );
    assert!(config.project_name.is_empty() && config.team_name.is_empty());
    h.set_secret("apifox_token", "synthetic-token");
    h.answer_next_prompt(Some(&["yes"]));
    h.on_fetch(|request| {
        assert!(request.url.contains("/projects/222/"));
        let body: Value = serde_json::from_slice(request.body.as_ref().unwrap()).unwrap();
        for key in [
            "targetEndpointFolderId",
            "targetSchemaFolderId",
            "targetBranchId",
            "moduleId",
        ] {
            assert!(body["options"].get(key).is_none(), "{body}");
        }
        Ok(openidea_plugin::HttpResponse {
            status: 200,
            body: br#"{"data":{"counters":{"endpointCreated":1,"endpointUpdated":0}}}"#.to_vec(),
            ..Default::default()
        })
    });
    ApifoxHelper
        .run_command("upload", &h.invocation(), None)
        .unwrap();
    assert_eq!(h.requests().len(), 1);
}

#[test]
fn configure_same_project_preserves_associations_and_cancel_does_not_save() {
    let h = MockHost::install();
    let initial = serde_json::to_string(&stored_config()).unwrap();
    host::storage::set("config:/project", &initial).unwrap();
    h.answer_next_prompt(Some(&[" 111 "]));
    h.answer_next_prompt(Some(&["11"]));
    ApifoxHelper
        .run_command("configure", &h.invocation(), None)
        .unwrap();
    assert_eq!(
        serde_json::to_value(load_config(&h.invocation()).unwrap()).unwrap(),
        serde_json::to_value(stored_config()).unwrap()
    );
    h.answer_next_prompt(Some(&["222"]));
    h.answer_next_prompt(None);
    ApifoxHelper
        .run_command("configure", &h.invocation(), None)
        .unwrap();
    assert_eq!(
        host::storage::get("config:/project").unwrap().unwrap(),
        initial
    );
}
