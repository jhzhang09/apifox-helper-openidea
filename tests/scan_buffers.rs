//! 全项目扫描不依赖标签页和版本化文档；未保存内容由宿主的文件读取契约提供。
use apifox_helper_openidea::plugin::scan;
use openidea_plugin::mock::MockHost;

fn controller(path: &str) -> String {
    format!(
        "@RestController class Api {{ @GetMapping(\"{path}\") public String get(){{return null;}} }}"
    )
}

#[test]
fn restored_tab_without_matching_versioned_document_does_not_break_scan() {
    let h = MockHost::install();
    h.add_file("src/Api.java", &controller("/saved"));
    h.open_editor("src/Api.java", "java", &controller("/saved"), &[]);
    // 标签路径能匹配，但版本化文档查询并不保证可用。
    let error = h
        .invocation()
        .project()
        .unwrap()
        .document("/project/src/Api.java")
        .unwrap_err();
    assert!(error.to_string().contains("is not open"));
    let export = scan(&h.invocation(), "project").unwrap();
    assert!(export.document["paths"].get("/saved").is_some());
}

#[test]
fn full_project_scan_reads_closed_files_without_consulting_editor_state() {
    let h = MockHost::install().with_capabilities(&["project.read", "storage"]);
    h.add_file("one/Api.java", &controller("/one"));
    h.add_file("two/Api.java", &controller("/two"));
    h.select_paths(&["one/Api.java"]);
    let export = scan(&h.invocation(), "project").unwrap();
    assert_eq!(export.endpoints.len(), 2);
    assert!(export.document["paths"].get("/one").is_some());
    assert!(export.document["paths"].get("/two").is_some());
}

#[test]
fn project_switch_and_switch_back_keep_each_projects_own_files_and_buffers() {
    let h = MockHost::install();
    h.add_file("/projects/first/src/Api.java", &controller("/first-saved"));
    h.add_file(
        "/projects/second/src/Api.java",
        &controller("/second-saved"),
    );
    h.set_project_root(Some("/projects/first"));
    h.open_editor(
        "/projects/first/src/Api.java",
        "java",
        &controller("/first-unsaved"),
        &[],
    );
    assert!(
        scan(&h.invocation(), "project").unwrap().document["paths"]
            .get("/first-unsaved")
            .is_some()
    );
    h.set_project_root(Some("/projects/second"));
    h.select_paths(&["/projects/first/src/Api.java"]);
    let second = scan(&h.invocation(), "project").unwrap();
    assert_eq!(second.endpoints.len(), 1);
    assert!(second.document["paths"].get("/second-saved").is_some());
    assert!(second.document["paths"].get("/first-unsaved").is_none());
    h.set_project_root(Some("/projects/first"));
    let first = scan(&h.invocation(), "project").unwrap();
    assert_eq!(first.endpoints.len(), 1);
    assert!(first.document["paths"].get("/first-unsaved").is_some());
}

#[test]
fn dto_buffers_are_read_even_when_the_controller_is_not_open() {
    let h = MockHost::install();
    h.add_file("src/Api.java", "package demo; @RestController class Api { @GetMapping(\"/user\") public User get(){return null;} }");
    h.add_file(
        "src/User.java",
        "package demo; class User { String saved; }",
    );
    h.open_editor(
        "/project/src/User.java",
        "java",
        "package demo; class User { String unsaved; }",
        &[],
    );
    let export = scan(&h.invocation(), "project").unwrap();
    assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    let schema = &export.document["components"]["schemas"]["User"];
    assert_eq!(schema["properties"]["unsaved"]["type"], "string");
    assert!(schema["properties"].get("saved").is_none());
}
