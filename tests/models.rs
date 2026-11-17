use apifox_helper_openidea::{
    openapi::{Scope, generate},
    schema::{Type, component_key},
    source,
};
use serde_json::{Value, json};
fn export(files: &[(&str, &str)]) -> apifox_helper_openidea::openapi::Export {
    let classes = files
        .iter()
        .flat_map(|(p, s)| source::parse(p, s))
        .collect::<Vec<_>>();
    generate(&classes, &Scope::default(), "测试")
}
fn follow<'a>(doc: &'a Value, mut s: &'a Value) -> &'a Value {
    for _ in 0..20 {
        if let Some(r) = s["$ref"].as_str() {
            s = doc.pointer(&r[1..]).expect("引用必须可解析");
        } else if let Some(a) = s["allOf"].as_array() {
            s = &a[0];
        } else {
            return s;
        }
    }
    panic!("引用循环")
}
#[test]
fn generic_nested_models_keep_actual_field_names_and_collections() {
    let result = export(&[(
        "Api.java",
        r#"package demo;
 @RestController @RequestMapping("/api") class Api {
 /** 获取用户 */ @GetMapping("/users") public ResponseEntity<Result<Page<User>>> list() { return null; }
 @GetMapping("/orders") public Result<Order> order() { return null; }
 }
 class Result<T> { /** 业务状态 */ private int code; private T payload; }
 class Page<T> { private List<T> rows; private long total; }
 class User { private String name; private Address address; private Map<String, List<Address>> contacts; }
 class Address { private String city; private Geo point; }
 class Geo { private double lat; private double lng; }
 class Order { private User buyer; }
 "#,
    )]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let doc = &result.document;
    let resp = follow(
        doc,
        &doc["paths"]["/api/users"]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
    );
    assert_eq!(resp["properties"]["code"]["type"], "integer");
    assert!(resp["properties"].get("data").is_none());
    let page = follow(doc, &resp["properties"]["payload"]);
    assert_eq!(page["properties"]["rows"]["type"], "array");
    let user = follow(doc, &page["properties"]["rows"]["items"]);
    let address = follow(doc, &user["properties"]["address"]);
    assert_eq!(address["properties"]["city"]["type"], "string");
    assert_eq!(
        follow(doc, &address["properties"]["point"])["properties"]["lat"]["type"],
        "number"
    );
    let map = &user["properties"]["contacts"];
    assert_eq!(map["additionalProperties"]["type"], "array");
    assert_eq!(
        follow(doc, &map["additionalProperties"]["items"])["properties"]["city"]["type"],
        "string"
    );
    let order = follow(
        doc,
        &doc["paths"]["/api/orders"]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
    );
    assert!(
        follow(doc, &order["properties"]["payload"])["properties"]
            .get("buyer")
            .is_some()
    );
}
#[test]
fn inheritance_cycles_serialization_and_validation() {
    let r = export(&[(
        "Api.java",
        r#"package p;
 @RestController class Api { @GetMapping("/node") public Node get() {return null;} }
 class Base<T> { private T value; }
 class Node extends Base<String> { @NotNull @JsonProperty("child_node") private Node child; @JsonIgnore private String secret; @Size(min=2,max=10) @Schema(description="节点名称",example="root") private String name; }
 "#,
    )]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let d = &r.document;
    let n = follow(
        d,
        &d["paths"]["/node"]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
    );
    assert_eq!(n["properties"]["value"]["type"], "string");
    assert!(n["properties"].get("secret").is_none());
    assert_eq!(n["required"], json!(["child_node"]));
    assert_eq!(n["properties"]["name"]["minLength"], 2);
    assert_eq!(n["properties"]["name"]["description"], "节点名称");
    assert_eq!(follow(d, &n["properties"]["child_node"]), n);
}
#[test]
fn packages_resolve_duplicate_simple_names() {
    let r = export(&[
        (
            "Api.java",
            "package api; import a.User; @RestController class Api { @GetMapping(\"/u\") public User get(){return null;} }",
        ),
        ("a/User.java", "package a; class User { private String a; }"),
        ("b/User.java", "package b; class User { private int b; }"),
    ]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let d = &r.document;
    assert!(
        follow(
            d,
            &d["paths"]["/u"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
        )["properties"]
            .get("a")
            .is_some()
    );
}
#[test]
fn request_attributes_are_not_client_parameters_and_json_body_is_preserved() {
    let r = export(&[(
        "Api.java",
        r#"@RestController @RequestMapping("/auth") class Api {
 @PostMapping("/bind") public void bind(@RequestAttribute DeviceContext context, @RequestBody @Valid BindRequest request, @RequestHeader("X-Id") String id) {}
 } class BindRequest { @NotBlank private String name; }"#,
    )]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let op = &r.document["paths"]["/auth/bind"]["post"];
    assert!(op["requestBody"]["content"]["application/json"].is_object());
    assert_eq!(op["parameters"].as_array().unwrap().len(), 1);
    assert_eq!(op["parameters"][0]["name"], "X-Id");
    assert!(op["responses"]["200"].get("content").is_none());
}
#[test]
fn kotlin_suspend_and_nested_data_classes() {
    let r = export(&[(
        "Api.kt",
        r#"package demo
 @RestController
 @RequestMapping("/kt")
 class Api {
 @GetMapping("/user")
 suspend fun get(@RequestParam(required = false) name: String?): Result<User> { TODO() }
 }
 data class Result<T>(val payload: T, val code: Int)
 data class User(val name: String, val address: Address)
 data class Address(val city: String)
 "#,
    )]);
    assert_eq!(r.endpoints.len(), 1, "{:?}", r);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let d = &r.document;
    let result = follow(
        d,
        &d["paths"]["/kt/user"]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
    );
    let user = follow(d, &result["properties"]["payload"]);
    assert_eq!(
        follow(d, &user["properties"]["address"])["properties"]["city"]["type"],
        "string"
    );
    assert_eq!(
        d["paths"]["/kt/user"]["get"]["parameters"][0]["required"],
        false
    );
}
#[test]
fn unresolved_types_warn_and_component_keys_cannot_collide() {
    let r = export(&[(
        "Api.java",
        "@RestController class Api { @GetMapping(\"/missing\") public Missing<User> get(){return null;} }",
    )]);
    assert!(!r.warnings.is_empty());
    assert_ne!(component_key("a.b.User"), component_key("a_b.User"));
    assert_eq!(
        Type::parse("Result<List<Map<String,User>>>").args[0].args[0].args[1].name,
        "User"
    );
}
#[test]
fn nested_static_dtos_resolve_within_enclosing_class() {
    let r = export(&[(
        "Api.java",
        r#"package p;
 @RestController class Api { @GetMapping("/nested") public Outer.Inner get(){return null;} }
 class Outer { static class Inner { private Address address; private List<Inner> children; } static class Address { private String city; } }
 "#,
    )]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let d = &r.document;
    let n = follow(
        d,
        &d["paths"]["/nested"]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
    );
    assert_eq!(
        follow(d, &n["properties"]["address"])["properties"]["city"]["type"],
        "string"
    );
    assert_eq!(n["properties"]["children"]["type"], "array");
}
#[test]
fn jaxrs_body_headers_and_response_media() {
    let r = export(&[(
        "Api.java",
        r#"@Path("/rest") class Api { @POST @Path("/items") @Produces("application/json; charset=UTF-8") public User create(@HeaderParam("X-Id") String id, User body){return null;} } class User { private String name; }"#,
    )]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let op = &r.document["paths"]["/rest/items"]["post"];
    assert!(op["requestBody"]["content"]["application/json"].is_object());
    assert!(op["responses"]["200"]["content"]["application/json"].is_object());
    assert_eq!(op["parameters"][0]["in"], "header");
}
#[test]
fn multipart_arrays_and_multiple_mappings() {
    let r = export(&[(
        "Api.java",
        r#"@RestController class Api { @PostMapping("/upload") public void upload(@RequestPart("files") MultipartFile[] files, @RequestParam(required=false) String note){} @RequestMapping(path={"/one","/two"},method={RequestMethod.GET,RequestMethod.POST}) public String multi(){return "";} }"#,
    )]);
    assert_eq!(r.endpoints.len(), 5);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let s = &r.document["paths"]["/upload"]["post"]["requestBody"]["content"]["multipart/form-data"]
        ["schema"]["properties"]["files"];
    assert_eq!(s["type"], "array");
    assert_eq!(s["items"]["format"], "binary");
}
#[test]
fn unresolved_route_constants_warn_instead_of_becoming_fake_urls() {
    let r = export(&[(
        "Api.java",
        r#"@RestController class Api { @GetMapping(Routes.USERS) public String get(){return "";} }"#,
    )]);
    assert!(r.endpoints.is_empty());
    assert!(r.warnings.iter().any(|s| s.contains("常量")));
}
#[test]
fn relative_literal_routes_and_multidimensional_arrays_remain_valid() {
    let r = export(&[(
        "Api.java",
        r#"@RequestMapping("base") @RestController class Api { @GetMapping("relative") public String[][] get(){return null;} }"#,
    )]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let s = &r.document["paths"]["/base/relative"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"];
    assert_eq!(s["items"]["items"]["type"], "string");
}
#[test]
fn partially_typed_declarations_do_not_crash_parser() {
    for code in [
        "class",
        "class Api<",
        "class Api<T>",
        "@",
        "enum",
        "interface",
        "record Api",
        r#"class Api<T> { @GetMapping("/u") public List<Map<String, T>> get(@RequestParam String name) { return null; } }"#,
    ] {
        for end in 0..=code.len() {
            if code.is_char_boundary(end) {
                let _ = source::parse("Api.java", &code[..end]);
            }
        }
    }
}

#[test]
fn type_use_annotations_do_not_truncate_collection_fields() {
    let result = export(&[(
        "Api.java",
        r#"
@RestController class Api {
 @PostMapping("/batch") public void batch(@RequestBody Request request) {}
}
class Request {
 @NotEmpty @Schema(description="标签列表",requiredMode=Schema.RequiredMode.REQUIRED)
 private List<@NotBlank(message="标签不能为空") String> labels;
 private Map<@NotBlank(message="键不能为空") String, @Valid Address> contacts;
}
class Address { private String city; }
"#,
    )]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let d = &result.document;
    let body = follow(
        d,
        &d["paths"]["/batch"]["post"]["requestBody"]["content"]["application/json"]["schema"],
    );
    assert_eq!(body["properties"]["labels"]["type"], "array");
    assert_eq!(body["properties"]["labels"]["items"]["type"], "string");
    assert!(body["properties"].get("message").is_none());
    assert_eq!(body["required"], json!(["labels"]));
    assert_eq!(
        follow(d, &body["properties"]["contacts"]["additionalProperties"])["properties"]["city"]["type"],
        "string"
    );
}

#[test]
fn raw_generics_and_map_superclasses_keep_open_shapes_without_resolution_errors() {
    let result = export(&[(
        "Api.java",
        r#"
@RestController class Api {
 @GetMapping("/raw") public R raw(){return null;}
 @GetMapping("/typed") public R<User> typed(){return null;}
 @GetMapping("/page") public Page page(){return null;}
 @GetMapping("/map") public DynamicResult map(){return null;}
}
class R<T>{private int code; private String msg; private T data;}
class Page<T>{private List<T> rows; private long total;}
class DynamicResult extends HashMap<String,Object>{private String traceId;}
class User{private String name;}
"#,
    )]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.notices.len(), 3);
    let d = &result.document;
    let response = |path: &str| {
        follow(
            d,
            &d["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
        )
    };
    assert_eq!(response("/raw")["properties"]["data"], json!({}));
    assert_eq!(
        follow(d, &response("/typed")["properties"]["data"])["properties"]["name"]["type"],
        "string"
    );
    assert_eq!(response("/page")["properties"]["rows"]["items"], json!({}));
    assert_eq!(response("/map")["additionalProperties"], json!({}));
    assert_eq!(response("/map")["properties"]["traceId"]["type"], "string");
}

#[test]
fn disabled_controllers_do_not_shadow_real_routes_but_real_conflicts_remain_errors() {
    let result = export(&[(
        "Api.java",
        r#"
// @RestController
class DisabledController { @GetMapping("/captchaEnabled") public String get(){return "";} }
@RestController class ActiveController { @GetMapping("/captchaEnabled") public boolean get(){return true;} }
"#,
    )]);
    assert_eq!(result.endpoints.len(), 1);
    assert_eq!(result.endpoints[0].handler, "ActiveController.get");
    assert!(result.warnings.is_empty());
    let conflict = export(&[(
        "Api.java",
        r#"@RestController class A{ @GetMapping("/same") public void a(){} } @RestController class B{ @GetMapping("/same") public void b(){} }"#,
    )]);
    assert!(conflict.warnings.iter().any(|w| w.contains("重复路由")));
}

#[test]
fn optional_path_variables_only_appear_in_paths_that_declare_them() {
    let r = export(&[(
        "Api.java",
        r#"@RestController @RequestMapping("/system/user") class Api {
 @GetMapping(value={"/","/{userId}"}) public String info(@PathVariable(value="userId",required=false) Long userId){return "";}
}"#,
    )]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!(
        r.document["paths"]["/system/user"]["get"]["parameters"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let parameter = &r.document["paths"]["/system/user/{userId}"]["get"]["parameters"][0];
    assert_eq!(parameter["name"], "userId");
    assert_eq!(parameter["required"], true);
}

#[test]
fn exported_model_names_follow_simple_classes_and_readable_generic_forms() {
    let r = export(&[(
        "Api.java",
        r#"package example; @RestController class Api {
 @GetMapping("/dto") public BdsCoordinate dto(){return null;}
 @GetMapping("/single") public R<String> single(){return null;}
 @GetMapping("/list") public R<List<ErpLocationVo>> list(){return null;}
 @GetMapping("/wrapped") public ApiBaseResponse<List<String>> wrapped(){return null;}
} class R<T>{private T data;} class ApiBaseResponse<T>{private T body;}
class ErpLocationVo{private BdsCoordinate position;}
class BdsCoordinate{private double latitude;private double longitude;}
"#,
    )]);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let schemas = r.document["components"]["schemas"].as_object().unwrap();
    for name in [
        "BdsCoordinate",
        "RString",
        "RListErpLocationVo",
        "ErpLocationVo",
        "ApiBaseResponseListString",
    ] {
        assert!(schemas.contains_key(name), "{name}");
        assert_eq!(schemas[name]["title"], name);
    }
    assert!(schemas.keys().all(|s| !s.contains("_2e_")));
    assert_eq!(
        r.document["paths"]["/list"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["$ref"],
        "#/components/schemas/RListErpLocationVo"
    );
    assert_eq!(
        schemas["RListErpLocationVo"]["properties"]["data"]["items"]["$ref"],
        "#/components/schemas/ErpLocationVo"
    );
    assert_eq!(
        r.endpoints
            .iter()
            .find(|e| e.path == "/list")
            .unwrap()
            .operation,
        r.document["paths"]["/list"]["get"]
    );
}
#[test]
fn readable_names_disambiguate_packages_and_generic_name_collisions_deterministically() {
    let files = [
        (
            "Api.java",
            r#"package api; @RestController class Api{
 @GetMapping("/a") public a.User a(){return null;}
 @GetMapping("/b") public b.User b(){return null;}
 @GetMapping("/r-a") public R<a.User> ra(){return null;}
 @GetMapping("/r-b") public R<b.User> rb(){return null;}
 @GetMapping("/real") public RUser real(){return null;}
} class R<T>{private T data;} class RUser{private int real;}
"#,
        ),
        (
            "a/User.java",
            "package a; class User{private String a; private User child;}",
        ),
        ("b/User.java", "package b; class User{private int b;}"),
    ];
    let r = export(&files);
    assert!(r.warnings.is_empty());
    let s = &r.document["components"]["schemas"];
    let schema = |path: &str| {
        &r.document["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
    };
    assert_ne!(schema("/a"), schema("/b"));
    assert_ne!(schema("/r-a"), schema("/r-b"));
    assert!(
        follow(&r.document, schema("/a"))["properties"]
            .get("a")
            .is_some()
    );
    assert!(
        follow(&r.document, schema("/b"))["properties"]
            .get("b")
            .is_some()
    );
    assert!(s.get("RUser").is_some());
    let classes = files
        .iter()
        .rev()
        .flat_map(|(p, s)| source::parse(p, s))
        .collect::<Vec<_>>();
    let again = generate(&classes, &Scope::default(), "测试");
    assert_eq!(again.document["components"], r.document["components"]);
    let scoped = generate(
        &classes,
        &Scope {
            method_line: Some(("Api.java".into(), 2)),
            ..Default::default()
        },
        "测试",
    );
    assert_eq!(scoped.document["paths"]["/b"], r.document["paths"]["/b"]);
}
#[test]
fn property_order_matches_source_with_nested_refs_renames_and_inheritance() {
    let r = export(&[(
        "Api.java",
        r#"package demo; @RestController class Api{ @GetMapping("/fence") public Fence fence(){return null;} }
class Base{private String createBy;}
class Fence extends Base{
 private List<BdsCoordinate> gpsTopPoints;
 private BdsCoordinate gpsCenterPoint;
 @JsonProperty("leftPoint") private BdsCoordinate gpsTopLeftPoint;
 private BdsCoordinate gpsBottomRightPoint;
 private String beginDate;
 @JsonIgnore private String ignored;
 private String endDate;
 private String beginTime;
}
class BdsCoordinate{private double latitude;private double longitude;private Double altitude;}
"#,
    )]);
    assert!(r.warnings.is_empty());
    let s = &r.document["components"]["schemas"];
    let fence = &s["Fence"];
    let order = vec![
        "createBy",
        "gpsTopPoints",
        "gpsCenterPoint",
        "leftPoint",
        "gpsBottomRightPoint",
        "beginDate",
        "endDate",
        "beginTime",
    ];
    assert_eq!(fence["x-apifox-orders"], json!(order));
    assert_eq!(
        fence["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        order
    );
    assert_eq!(
        fence["properties"]["gpsTopPoints"]["items"]["$ref"],
        "#/components/schemas/BdsCoordinate"
    );
    assert_eq!(
        fence["properties"]["gpsCenterPoint"]["allOf"][0]["$ref"],
        "#/components/schemas/BdsCoordinate"
    );
    assert_eq!(
        s["BdsCoordinate"]["x-apifox-orders"],
        json!(["latitude", "longitude", "altitude"])
    );
}
