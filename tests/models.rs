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

#[test]
fn resolves_route_constants_and_plus_concatenation_across_java_and_kotlin() {
    let result = export(&[
        (
            "src/main/java/demo/ApiRoutes.java",
            r#"
package demo;
public interface ApiRoutes {
    String PREFIX = "/api";
    String USERS = "/users";
}
"#,
        ),
        (
            "src/main/kotlin/demo/KtRoutes.kt",
            r#"
package demo
object KtRoutes {
    const val V1 = "/v1"
}
class KtConstants {
    companion object {
        const val ITEMS = "/items"
    }
}
"#,
        ),
        (
            "src/main/java/demo/MixedController.java",
            r#"
package demo;
import static demo.ApiRoutes.PREFIX;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping(PREFIX + KtRoutes.V1)
public class MixedController {
    private static final String DETAIL = "/detail";

    @GetMapping(ApiRoutes.USERS + DETAIL)
    public String listUsers() { return ""; }

    @PostMapping(KtConstants.ITEMS + "/create")
    public String createItem() { return ""; }
}
"#,
        ),
    ]);

    assert!(
        result.warnings.is_empty(),
        "warnings: {:?}",
        result.warnings
    );
    let paths: Vec<&str> = result.endpoints.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"/api/v1/users/detail"), "paths: {paths:?}");
    assert!(paths.contains(&"/api/v1/items/create"), "paths: {paths:?}");
}

#[test]
fn inherited_overloads_resolve_in_their_declaration_package() {
    let result = export(&[
        (
            "a/Base.java",
            "package a; public class Base<T> {\n@GetMapping(\"/text\") public Item find(String id){return null;}\n@GetMapping(\"/typed\") public T find(T id){return null;} }",
        ),
        (
            "a/Item.java",
            "package a; public class Item { public String expected; }",
        ),
        (
            "b/Item.java",
            "package b; public class Item { public String other; }",
        ),
        (
            "b/C.java",
            "package b; import a.Base; @RestController public class C extends Base<Item> {\n@Override public Item find(Item id){return null;} }",
        ),
    ]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.endpoints.len(), 2);
    for (path, field, declaration) in [
        ("/text", "expected", Some("a/Base.java")),
        ("/typed", "other", None),
    ] {
        let endpoint = result.endpoints.iter().find(|ep| ep.path == path).unwrap();
        assert_eq!(endpoint.file, "b/C.java");
        assert_eq!(endpoint.declaration_file.as_deref(), declaration);
        assert_eq!(endpoint.line, 1);
        let schema =
            &endpoint.operation["responses"]["200"]["content"]["application/json"]["schema"];
        assert!(
            follow(&result.document, schema)["properties"]
                .get(field)
                .is_some()
        );
    }
}

#[test]
fn merges_controller_interface_and_generic_superclass_endpoints() {
    let result = export(&[
        (
            "src/main/java/demo/OrderDto.java",
            r#"
package demo;
public class OrderDto {
    /** 订单号 */
    public String orderNo;
}
"#,
        ),
        (
            "src/main/java/demo/UserApi.java",
            r#"
package demo;
import org.springframework.web.bind.annotation.*;
@RequestMapping("/api/users")
public interface UserApi {
    /** 根据编号查用户 */
    @GetMapping("/{id}")
    OrderDto findById(@PathVariable("id") String id);
}
"#,
        ),
        (
            "src/main/java/demo/UserApiImpl.java",
            r#"
package demo;
import org.springframework.web.bind.annotation.RestController;
@RestController
public class UserApiImpl implements UserApi {
    @Override
    public OrderDto findById(String id) { return null; }
}
"#,
        ),
        (
            "src/main/java/demo/BaseController.java",
            r#"
package demo;
import org.springframework.web.bind.annotation.*;
public abstract class BaseController<T> {
    @GetMapping("/{id}")
    public T detail(@PathVariable("id") Long id) { return null; }
}
"#,
        ),
        (
            "src/main/java/demo/OrderController.java",
            r#"
package demo;
import org.springframework.web.bind.annotation.*;
@RestController
@RequestMapping("/api/orders")
public class OrderController extends BaseController<OrderDto> {}
"#,
        ),
    ]);

    assert!(
        result.warnings.is_empty(),
        "warnings: {:?}",
        result.warnings
    );
    let user_ep = result
        .endpoints
        .iter()
        .find(|e| e.path == "/api/users/{id}")
        .expect("inherited interface endpoint");
    assert_eq!(user_ep.operation["summary"], "根据编号查用户");
    assert_eq!(user_ep.operation["parameters"][0]["in"], "path");
    assert_eq!(user_ep.operation["parameters"][0]["name"], "id");

    let order_ep = result
        .endpoints
        .iter()
        .find(|e| e.path == "/api/orders/{id}")
        .expect("generic base endpoint");
    let schema = &order_ep.operation["responses"]["200"]["content"]["application/json"]["schema"];
    let resolved = follow(&result.document, schema);
    assert!(
        resolved["properties"].get("orderNo").is_some(),
        "resolved: {resolved}"
    );
}

#[test]
fn extracts_enum_json_value_descriptions_and_jackson_class_annotations() {
    let result = export(&[
        (
            "src/main/java/demo/OrderStatus.java",
            r#"
package demo;
import com.fasterxml.jackson.annotation.JsonValue;
public enum OrderStatus {
    /** 待支付 */
    PENDING(1),
    /** 已完成 */
    DONE(2);

    @JsonValue
    private final int code;
    OrderStatus(int code) { this.code = code; }
}
"#,
        ),
        (
            "src/main/java/demo/SnakeBody.java",
            r#"
package demo;
import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import com.fasterxml.jackson.databind.PropertyNamingStrategies;
import com.fasterxml.jackson.databind.annotation.JsonNaming;
@JsonIgnoreProperties({"internalSecret"})
@JsonNaming(PropertyNamingStrategies.SnakeCaseStrategy.class)
public class SnakeBody {
    /** 名 */
    public String firstName;
    public String internalSecret;
    public OrderStatus status;
}
"#,
        ),
        (
            "src/main/java/demo/SnakeController.java",
            r#"
package demo;
import org.springframework.web.bind.annotation.*;
@RestController
@RequestMapping("/snake")
public class SnakeController {
    @PostMapping("/test")
    public SnakeBody test(@RequestBody SnakeBody body) { return body; }
}
"#,
        ),
    ]);

    assert!(
        result.warnings.is_empty(),
        "warnings: {:?}",
        result.warnings
    );
    let ep = &result.endpoints[0];
    let body_schema = follow(
        &result.document,
        &ep.operation["requestBody"]["content"]["application/json"]["schema"],
    );
    let snake_props = body_schema["properties"]
        .as_object()
        .expect("SnakeBody props");
    assert!(
        snake_props.contains_key("first_name"),
        "props: {snake_props:?}"
    );
    assert!(
        !snake_props.contains_key("internalSecret") && !snake_props.contains_key("internal_secret")
    );

    let status_schema = follow(&result.document, &snake_props["status"]);
    assert_eq!(status_schema["type"], "integer");
    assert_eq!(status_schema["enum"], json!([1, 2]));
    let x_enum = status_schema["x-apifox-enum"].to_string();
    assert!(
        x_enum.contains("待支付") && x_enum.contains("已完成"),
        "x-apifox-enum: {x_enum}"
    );
}

#[test]
fn supports_operation_tags_header_parameter_and_model_attribute_descriptions() {
    let result = export(&[
        (
            "src/main/java/demo/SearchQuery.java",
            r#"
package demo;
public class SearchQuery {
    /** 搜索关键词 */
    public String keyword;
}
"#,
        ),
        (
            "src/main/java/demo/TagController.java",
            r#"
package demo;
import io.swagger.v3.oas.annotations.Operation;
import io.swagger.v3.oas.annotations.Parameter;
import io.swagger.v3.oas.annotations.enums.ParameterIn;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/items")
public class TagController {
    @Operation(summary = "查询条目", tags = {"条目管理", "公开接口"})
    @GetMapping
    public String list(
        @Parameter(name = "X-Trace-Id", in = ParameterIn.HEADER, description = "链路追踪ID") String traceId,
        @ModelAttribute SearchQuery query
    ) {
        return "";
    }
}
"#,
        ),
    ]);

    assert!(
        result.warnings.is_empty(),
        "warnings: {:?}",
        result.warnings
    );
    let ep = &result.endpoints[0];
    assert_eq!(ep.operation["tags"], json!(["条目管理", "公开接口"]));
    let params = ep.operation["parameters"].as_array().expect("parameters");
    let header_p = params
        .iter()
        .find(|p| p["name"] == "X-Trace-Id")
        .expect("header param");
    assert_eq!(header_p["in"], "header");
    assert_eq!(header_p["description"], "链路追踪ID");
    let query_p = params
        .iter()
        .find(|p| p["name"] == "keyword")
        .expect("query param");
    assert_eq!(query_p["in"], "query");
    assert_eq!(query_p["description"], "搜索关键词");
}

#[test]
fn supports_java_text_blocks_in_bodies_and_annotations_and_preserves_line_numbers() {
    let java_code = concat!(
        "package demo;\n",
        "import io.swagger.v3.oas.annotations.Operation;\n",
        "import org.springframework.web.bind.annotation.*;\n",
        "@RestController\n",
        "public class PolicyController {\n",
        "    private String sample() {\n",
        "        return switch (1) {\n",
        "            case 1 -> \"\"\"\n",
        "                {\n",
        "                  \"Version\": \"2012-10-17\",\n",
        "                  \"Escaped\": \"\\\"\\\"\\\"\",\n",
        "                  \"Backslash\": \"\\\\\",\n",
        "                  \"Continued\": \"hello\" \\\n",
        "                    \"world\"\n",
        "                }\n",
        "                \"\"\";\n",
        "            default -> \"\";\n",
        "        };\n",
        "    }\n",
        "    @Operation(summary = \"获取策略\", description = \"\"\"\n",
        "        第一行说明\n",
        "        第二行说明\n",
        "        \"\"\")\n",
        "    @GetMapping(\"/policy\")\n",
        "    public String policy() { return sample(); }\n",
        "}\n",
    );

    let classes = source::parse_checked("src/main/java/demo/PolicyController.java", java_code)
        .expect("text blocks must parse cleanly");
    assert_eq!(classes.len(), 1);
    let policy_method = classes[0]
        .methods
        .iter()
        .find(|m| m.name == "policy")
        .expect("policy method");
    assert_eq!(policy_method.line, 19);

    let result = export(&[("src/main/java/demo/PolicyController.java", java_code)]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.endpoints.len(), 1);
    assert_eq!(
        result.endpoints[0].operation["description"],
        "第一行说明\n第二行说明"
    );
}

#[test]
fn supports_kotlin_raw_strings_nested_comments_and_modern_modifiers() {
    let kt_code = r#"
package demo
import org.springframework.web.bind.annotation.*
/* outer /* nested } */ still comment { */
@JvmInline
value class UserId(val value: Long)
data object Constants {
    const val BASE = "/kt"
}
fun interface Mapper {
    fun map(v: String): String
}
@RestController
@RequestMapping(Constants.BASE)
class ModernController {
    inner class Helper {
        val raw = """C:\path\to\dir\"""
        val trailingQuotes = """hello""""
    }
    @GetMapping("/user")
    fun user(): UserId? = null
}
"#;
    let kt_classes = source::parse_checked("src/main/kotlin/demo/Modern.kt", kt_code)
        .expect("kotlin raw strings and nested comments must parse cleanly");
    assert!(kt_classes.iter().any(|c| c.name == "UserId"));
    assert!(kt_classes.iter().any(|c| c.name == "Constants"));
    assert!(kt_classes.iter().any(|c| c.name == "Mapper"));

    let java_modifiers = r#"
package demo;
import org.springframework.web.bind.annotation.*;
public sealed interface Shape permits Circle {}
public non-sealed class Circle implements Shape {
    public String name;
}
public record Point(int x, int y) {}
@RestController
public class ShapeController {
    @GetMapping("/circle")
    public Circle circle() { return null; }
    @GetMapping("/point")
    public Point point() { return null; }
}
"#;
    let result = export(&[
        ("src/main/kotlin/demo/Modern.kt", kt_code),
        ("src/main/java/demo/Shapes.java", java_modifiers),
    ]);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let paths = result.document["paths"].as_object().unwrap();
    assert!(paths.contains_key("/kt/user"));
    assert!(paths.contains_key("/circle"));
    assert!(paths.contains_key("/point"));
    let schemas = result.document["components"]["schemas"]
        .as_object()
        .unwrap();
    assert_eq!(schemas["Circle"]["properties"]["name"]["type"], "string");
    assert_eq!(schemas["Point"]["properties"]["x"]["format"], "int32");
}

#[test]
fn handles_injected_session_catch_all_routes_resource_json_and_pair_without_shadowing_local() {
    let result = export(&[
        (
            "src/main/java/demo/ApiController.java",
            r#"
package demo;
import cn.hutool.core.lang.Dict;
import cn.hutool.json.JSONArray;
import com.alibaba.fastjson2.JSONObject;
import com.fasterxml.jackson.databind.JsonNode;
import jakarta.servlet.http.HttpSession;
import org.springframework.core.io.Resource;
import org.springframework.data.util.Pair;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.*;
import org.springframework.web.server.ServerWebExchange;

@RestController
@RequestMapping("/files")
public class ApiController {
    @GetMapping("/view/{*filename}")
    public ResponseEntity<Resource> download(
        @PathVariable String filename,
        HttpSession session,
        ServerWebExchange exchange
    ) { return null; }

    @PostMapping("/payload")
    public Pair<String, Integer> payload(
        @RequestBody PayloadDto body
    ) { return null; }
}
class PayloadDto {
    public JSONObject obj;
    public JSONArray arr;
    public Dict dict;
    public JsonNode node;
}
"#,
        ),
        (
            "src/main/java/local/LocalController.java",
            r#"
package local;
import org.springframework.web.bind.annotation.*;
@RestController
public class LocalController {
    @GetMapping("/local-resource")
    public Resource local() { return null; }
}
class Resource {
    public String customField;
}
"#,
        ),
    ]);

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert!(!result.notices.is_empty());

    let download = &result.document["paths"]["/files/view/{filename}"]["get"];
    assert!(download.is_object(), "catch-all path should normalize");
    let params = download["parameters"].as_array().unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["name"], "filename");
    assert_eq!(
        download["responses"]["200"]["content"]["application/json"]["schema"]["format"],
        "binary"
    );

    let payload_res = &result.document["paths"]["/files/payload"]["post"]["responses"]["200"]["content"]
        ["application/json"]["schema"];
    assert_eq!(payload_res["properties"]["first"]["type"], "string");
    assert_eq!(payload_res["properties"]["second"]["format"], "int32");

    let schemas = result.document["components"]["schemas"]
        .as_object()
        .unwrap();
    let payload_dto = &schemas["PayloadDto"]["properties"];
    assert_eq!(payload_dto["obj"]["type"], "object");
    assert_eq!(payload_dto["obj"]["additionalProperties"], json!({}));
    assert_eq!(payload_dto["arr"]["type"], "array");
    assert_eq!(payload_dto["dict"]["type"], "object");
    assert_eq!(payload_dto["node"], json!({}));

    assert_eq!(
        schemas["Resource"]["properties"]["customField"]["type"],
        "string"
    );
}

#[test]
fn spring_consumes_overloads_merge_into_single_operation_without_blocking() {
    let result = export(&[(
        "PluginCommandController.java",
        r#"
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.*;
import org.springframework.web.server.ServerWebExchange;
import io.swagger.v3.oas.annotations.Operation;
import reactor.core.publisher.Mono;
import java.util.Map;

@RestController
@RequestMapping("/plugin/command")
public class PluginCommandController {
    @PostMapping(value = "/{type}/{pluginId}/{commandId}", consumes = MediaType.APPLICATION_JSON_VALUE)
    @Operation(summary = "POST方式执行插件命令(Json)")
    public Mono<ResponseEntity<Object>> callCommandPost(
        @PathVariable String type,
        @PathVariable String pluginId,
        @PathVariable String commandId,
        @RequestParam(required = false) Map<String, Object> pathParam,
        @RequestBody Mono<Map<String, Object>> paramsMono
    ) { return null; }

    @PostMapping(value = "/{type}/{pluginId}/{commandId}", consumes = MediaType.APPLICATION_FORM_URLENCODED_VALUE)
    @Operation(summary = "POST方式执行插件命令(Form)")
    public Mono<ResponseEntity<Object>> callCommandPost(
        @PathVariable String type,
        @PathVariable String pluginId,
        @PathVariable String commandId,
        ServerWebExchange exchange
    ) { return null; }
}
"#,
    )]);

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.endpoints.len(), 1);
    let op = &result.document["paths"]["/plugin/command/{type}/{pluginId}/{commandId}"]["post"];
    let content = op["requestBody"]["content"].as_object().unwrap();
    assert!(content.contains_key("application/json"), "{content:?}");
    assert!(
        content.contains_key("application/x-www-form-urlencoded"),
        "{content:?}"
    );
    assert_eq!(
        content["application/x-www-form-urlencoded"]["schema"]["type"],
        "object"
    );
    assert!(
        result.notices.iter().any(|n| n.contains("同路由条件重载")
            && n.contains("consumes")
            && n.contains("（PluginCommandController.java）")),
        "{:?}",
        result.notices
    );
    assert!(
        result.endpoints[0]
            .notices
            .iter()
            .any(|n| n.contains("同路由条件重载")),
        "{:?}",
        result.endpoints[0].notices
    );
}

#[test]
fn spring_consumes_overload_order_independent_form_then_json() {
    let result = export(&[(
        "OrderController.java",
        r#"
import org.springframework.http.MediaType;
import org.springframework.web.bind.annotation.*;
import org.springframework.web.server.ServerWebExchange;
import java.util.Map;

@RestController
@RequestMapping("/cmd")
public class OrderController {
    @PostMapping(value = "/run", consumes = MediaType.APPLICATION_FORM_URLENCODED_VALUE)
    public String run(ServerWebExchange exchange) { return null; }

    @PostMapping(value = "/run", consumes = MediaType.APPLICATION_JSON_VALUE)
    public String run(@RequestBody Map<String, Object> body) { return null; }
}
"#,
    )]);

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.endpoints.len(), 1);
    let content = result.document["paths"]["/cmd/run"]["post"]["requestBody"]["content"]
        .as_object()
        .unwrap();
    assert!(content.contains_key("application/x-www-form-urlencoded"));
    assert!(content.contains_key("application/json"));
}

#[test]
fn spring_produces_params_headers_overloads_merge_and_duplicate_condition_still_errors() {
    let result = export(&[(
        "VariantController.java",
        r#"
import org.springframework.http.MediaType;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/variants")
public class VariantController {
    @GetMapping(value = "/export", produces = MediaType.APPLICATION_JSON_VALUE)
    public String exportJson() { return null; }

    @GetMapping(value = "/export", produces = MediaType.TEXT_PLAIN_VALUE)
    public String exportText() { return null; }

    @GetMapping(value = "/search", params = "mode=fast")
    public String searchFast(@RequestParam String q) { return null; }

    @GetMapping(value = "/search", params = "mode=full")
    public String searchFull(@RequestParam String q, @RequestParam Integer page) { return null; }

    @GetMapping(value = "/versioned", headers = "X-Api-Version=1")
    public String v1() { return null; }

    @GetMapping(value = "/versioned", headers = "X-Api-Version=2")
    public String v2(@RequestHeader("X-Trace") String trace) { return null; }
}
"#,
    )]);

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.endpoints.len(), 3);
    let export_content =
        result.document["paths"]["/variants/export"]["get"]["responses"]["200"]["content"]
            .as_object()
            .unwrap();
    assert!(export_content.contains_key("application/json"));
    assert!(export_content.contains_key("text/plain"));

    let search_params = result.document["paths"]["/variants/search"]["get"]["parameters"]
        .as_array()
        .unwrap();
    assert_eq!(search_params.len(), 2);
    let page_param = search_params
        .iter()
        .find(|p| p["name"] == "page")
        .expect("merged param");
    assert_eq!(page_param["required"], false);

    let versioned_params = result.document["paths"]["/variants/versioned"]["get"]["parameters"]
        .as_array()
        .unwrap();
    assert_eq!(versioned_params.len(), 1);
    assert_eq!(versioned_params[0]["name"], "X-Trace");
    assert_eq!(versioned_params[0]["required"], false);

    assert!(result.notices.iter().any(|n| n.contains("produces")));
    assert!(result.notices.iter().any(|n| n.contains("params")));
    assert!(result.notices.iter().any(|n| n.contains("headers")));

    let conflict = export(&[(
        "TripleController.java",
        r#"
import org.springframework.http.MediaType;
import org.springframework.web.bind.annotation.*;
import org.springframework.web.server.ServerWebExchange;
import java.util.Map;

@RestController
@RequestMapping("/triple")
public class TripleController {
    @PostMapping(value = "/exec", consumes = MediaType.APPLICATION_JSON_VALUE)
    public String exec(@RequestBody Map<String, Object> body) { return null; }

    @PostMapping(value = "/exec", consumes = MediaType.APPLICATION_FORM_URLENCODED_VALUE)
    public String exec(ServerWebExchange exchange) { return null; }

    @PostMapping(value = "/exec", consumes = MediaType.APPLICATION_FORM_URLENCODED_VALUE)
    public String execDup(ServerWebExchange exchange) { return null; }
}
"#,
    )]);
    assert_eq!(conflict.endpoints.len(), 1);
    assert!(
        conflict
            .warnings
            .iter()
            .any(|w| w.contains("重复路由") && w.contains("（TripleController.java）")),
        "{:?}",
        conflict.warnings
    );
}

#[test]
fn webflux_file_part_map_entry_and_nested_explicit_import_resolve_cleanly() {
    let result = export(&[
        (
            "demo/Item.java",
            "package demo; public class Item { public String id; }",
        ),
        (
            "demo/Term.java",
            "package demo; public class Term { public enum Type { and, or } }",
        ),
        (
            "demo/Part.java",
            "package demo; public class Part { public String localField; }",
        ),
        (
            "api/UploadController.java",
            r#"
package api;
import demo.Item;
import demo.Term;
import org.springframework.http.codec.multipart.FilePart;
import org.springframework.web.bind.annotation.*;
import reactor.core.publisher.Flux;
import reactor.core.publisher.Mono;
import java.util.List;
import java.util.Map;

@RestController
@RequestMapping("/api")
public class UploadController {
    @PostMapping("/upload")
    public Mono<String> upload(@RequestPart("file") Mono<FilePart> file, Flux<FilePart> files) {
        return null;
    }

    @GetMapping("/entries")
    public List<Map.Entry<String, List<Item>>> entries() { return null; }

    @GetMapping("/term")
    public Term.Type termType() { return null; }

    @GetMapping("/local-part")
    public demo.Part localPart() { return null; }
}
"#,
        ),
    ]);

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let upload_content = &result.document["paths"]["/api/upload"]["post"]["requestBody"]["content"];
    assert!(
        upload_content["multipart/form-data"]["schema"]["properties"]["file"]["format"] == "binary",
        "{upload_content}"
    );
    assert!(
        upload_content["multipart/form-data"]["schema"]["properties"]["files"]["items"]["format"]
            == "binary",
        "{upload_content}"
    );
    let entry_schema = &result.document["paths"]["/api/entries"]["get"]["responses"]["200"]["content"]
        ["application/json"]["schema"]["items"];
    assert_eq!(entry_schema["type"], "object");
    assert_eq!(entry_schema["additionalProperties"]["type"], "array");
    assert_eq!(
        entry_schema["additionalProperties"]["items"]["$ref"],
        "#/components/schemas/Item"
    );
    let term_ref = &result.document["paths"]["/api/term"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["$ref"];
    assert_eq!(term_ref, "#/components/schemas/Type");
    assert_eq!(
        result.document["components"]["schemas"]["Type"]["enum"],
        json!(["and", "or"])
    );
    let local_part_ref = &result.document["paths"]["/api/local-part"]["get"]["responses"]["200"]["content"]
        ["application/json"]["schema"]["$ref"];
    assert_eq!(local_part_ref, "#/components/schemas/Part");
    assert_eq!(
        result.document["components"]["schemas"]["Part"]["properties"]["localField"]["type"],
        "string"
    );
}

#[test]
fn conditional_route_overloads_merge_into_single_operation_without_blocking_errors() {
    let result = export(&[(
        "PluginController.java",
        r#"
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.*;
import org.springframework.web.server.ServerWebExchange;
import reactor.core.publisher.Mono;
import java.util.Map;

@RestController
@RequestMapping("/plugin/command")
public class PluginController {
    @PostMapping(value = "/{type}/{pluginId}/{commandId}", consumes = MediaType.APPLICATION_JSON_VALUE)
    @Operation(summary = "POST方式执行插件命令(Json)")
    public Mono<ResponseEntity<Object>> callCommandPost(
            @PathVariable String type,
            @PathVariable String pluginId,
            @PathVariable String commandId,
            @RequestParam(required = false) Map<String, Object> pathParam,
            @RequestBody Mono<Map<String, Object>> paramsMono) {
        return null;
    }

    @PostMapping(value = "/{type}/{pluginId}/{commandId}", consumes = MediaType.APPLICATION_FORM_URLENCODED_VALUE)
    @Operation(summary = "POST方式执行插件命令(Form)")
    public Mono<ResponseEntity<Object>> callCommandPost(
            @PathVariable String type,
            @PathVariable String pluginId,
            @PathVariable String commandId,
            ServerWebExchange exchange) {
        return null;
    }
}
"#,
    )]);

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.endpoints.len(), 1);
    assert!(
        result
            .notices
            .iter()
            .any(|n| n.contains("同路由条件重载") && n.contains("PluginController.java")),
        "{:?}",
        result.notices
    );
    let post = &result.document["paths"]["/plugin/command/{type}/{pluginId}/{commandId}"]["post"];
    let content = post["requestBody"]["content"].as_object().unwrap();
    assert!(content.contains_key("application/json"), "{content:?}");
    assert!(
        content.contains_key("application/x-www-form-urlencoded"),
        "{content:?}"
    );
    assert_eq!(
        content["application/x-www-form-urlencoded"]["schema"]["type"],
        "object"
    );
    assert_eq!(post["requestBody"]["required"], false);
}

#[test]
fn conditional_overloads_support_produces_params_headers_and_reverse_order() {
    let result = export(&[(
        "ConditionalApi.java",
        r#"
import org.springframework.http.MediaType;
import org.springframework.web.bind.annotation.*;
import org.springframework.web.server.ServerWebExchange;

@RestController
@RequestMapping("/cond")
public class ConditionalApi {
    @PostMapping(value = "/submit", consumes = MediaType.APPLICATION_FORM_URLENCODED_VALUE)
    public String submitForm(ServerWebExchange exchange) { return ""; }

    @PostMapping(value = "/submit", consumes = MediaType.APPLICATION_JSON_VALUE)
    public String submitJson(@RequestBody Payload body) { return ""; }

    @GetMapping(value = "/data", produces = MediaType.APPLICATION_JSON_VALUE)
    public Payload dataJson() { return null; }

    @GetMapping(value = "/data", produces = MediaType.APPLICATION_XML_VALUE)
    public String dataXml() { return ""; }

    @GetMapping(value = "/query", params = "mode=fast")
    public String queryFast(@RequestParam String mode) { return ""; }

    @GetMapping(value = "/query", params = "mode=full", headers = "X-Trace=1")
    public String queryFull(@RequestParam String mode, @RequestHeader("X-Trace") String trace) { return ""; }
}
class Payload { public String name; }
"#,
    )]);

    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.endpoints.len(), 3);

    let submit_content = result.document["paths"]["/cond/submit"]["post"]["requestBody"]["content"]
        .as_object()
        .unwrap();
    assert!(submit_content.contains_key("application/x-www-form-urlencoded"));
    assert!(submit_content.contains_key("application/json"));

    let data_content = result.document["paths"]["/cond/data"]["get"]["responses"]["200"]["content"]
        .as_object()
        .unwrap();
    assert!(data_content.contains_key("application/json"));
    assert!(data_content.contains_key("application/xml"));

    let query_params = result.document["paths"]["/cond/query"]["get"]["parameters"]
        .as_array()
        .unwrap();
    assert!(query_params.iter().any(|p| p["name"] == "mode"));
    assert!(
        query_params
            .iter()
            .any(|p| p["name"] == "X-Trace" && p["required"] == false)
    );
}

#[test]
fn overlapping_conditional_overload_still_reports_duplicate_route_error() {
    let result = export(&[(
        "ConflictController.java",
        r#"
import org.springframework.http.MediaType;
import org.springframework.web.bind.annotation.*;
import org.springframework.web.server.ServerWebExchange;

@RestController
@RequestMapping("/plugin/command")
public class ConflictController {
    @PostMapping(value = "/exec", consumes = MediaType.APPLICATION_JSON_VALUE)
    public void execJson(@RequestBody String body) {}

    @PostMapping(value = "/exec", consumes = MediaType.APPLICATION_FORM_URLENCODED_VALUE)
    public void execForm(ServerWebExchange exchange) {}

    @PostMapping(value = "/exec", consumes = MediaType.APPLICATION_FORM_URLENCODED_VALUE)
    public void execFormDup(ServerWebExchange exchange) {}
}
"#,
    )]);

    assert_eq!(result.endpoints.len(), 1);
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert!(result.warnings[0].contains("重复路由"));
    assert!(result.warnings[0].contains("ConflictController.java"));
}

#[test]
fn resolves_lombok_kotlin_delegating_builtin_and_variant_override_json_value_enums() {
    let result = export(&[
        (
            "src/main/java/demo/CoordUpdateReq.java",
            r#"
package demo;
import com.fasterxml.jackson.annotation.JsonValue;
import lombok.AllArgsConstructor;
import lombok.Getter;
import lombok.RequiredArgsConstructor;

public class CoordUpdateReq {
    public CoordType coordType;
    public RequiredCode requiredCode;
    public NameLowerEnum nameLower;
    public NameUpperEnum nameUpper;
    public OrdinalEnum ordinalEnum;
    public DelegatingEnum delegatingEnum;
    public DualAnnotatedEnum dualAnnotated;
    public VariantOverrideEnum variantOverride;

    @Getter
    @AllArgsConstructor
    public enum CoordType {
        WGS84("wgs84", "GPS坐标"),
        GCJ02("gcj02", "火星坐标");
        @JsonValue
        private final String code;
        private final String desc;
    }

    @RequiredArgsConstructor
    public enum RequiredCode {
        FAST(1),
        SLOW(2);
        @JsonValue
        private final int code;
        private String mutableNote;
    }

    public enum NameLowerEnum {
        ALPHA, BETA;
        @JsonValue
        public String toValue() { return name().toLowerCase(java.util.Locale.ROOT); }
    }

    public enum NameUpperEnum {
        first, second;
        @JsonValue
        public String toValue() { return this.name().toUpperCase(); }
    }

    public enum OrdinalEnum {
        ZERO, ONE;
        @JsonValue
        public int indexVal() { return ordinal(); }
    }

    public enum DelegatingEnum {
        A("a"),
        B("b", "B说明");
        @JsonValue
        private final String code;
        private final String desc;
        DelegatingEnum(String code) { this(code, ""); }
        DelegatingEnum(String code, String desc) {
            java.util.Objects.requireNonNull(code);
            this.code = code;
            this.desc = desc == null ? "" : desc;
        }
        @JsonValue
        public String value() { return getCode(); }
        public String getCode() { return code; }
    }

    public enum DualAnnotatedEnum {
        X(10), Y(20);
        @JsonValue
        private final int code;
        DualAnnotatedEnum(int code) { this.code = code; }
        @JsonValue
        public int getCode() { return this.code; }
    }

    public enum VariantOverrideEnum {
        UP { @Override public String getCode() { return "up"; } },
        DOWN { @Override public String getCode() { return "down"; } };
        @JsonValue
        public abstract String getCode();
    }
}
"#,
        ),
        (
            "src/main/kotlin/demo/KtEnum.kt",
            r#"
package demo
import com.fasterxml.jackson.annotation.JsonValue

enum class KtEnum(@JsonValue val code: String) {
    ON("on"),
    OFF("off");

    @JsonValue
    fun asJson(): String = code
}
"#,
        ),
        (
            "src/main/java/demo/EnumApi.java",
            r#"
package demo;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/enums")
public class EnumApi {
    @PostMapping("/check")
    public KtEnum check(@RequestBody CoordUpdateReq req) { return null; }
}
"#,
        ),
    ]);

    assert!(
        result.warnings.is_empty(),
        "warnings: {:?}",
        result.warnings
    );
    let schemas = &result.document["components"]["schemas"];
    assert_eq!(schemas["CoordType"]["type"], "string");
    assert_eq!(schemas["CoordType"]["enum"], json!(["wgs84", "gcj02"]));
    assert_eq!(schemas["RequiredCode"]["type"], "integer");
    assert_eq!(schemas["RequiredCode"]["enum"], json!([1, 2]));
    assert_eq!(schemas["NameLowerEnum"]["enum"], json!(["alpha", "beta"]));
    assert_eq!(schemas["NameUpperEnum"]["enum"], json!(["FIRST", "SECOND"]));
    assert_eq!(schemas["OrdinalEnum"]["type"], "integer");
    assert_eq!(schemas["OrdinalEnum"]["enum"], json!([0, 1]));
    assert_eq!(schemas["DelegatingEnum"]["enum"], json!(["a", "b"]));
    assert_eq!(schemas["DualAnnotatedEnum"]["enum"], json!([10, 20]));
    assert_eq!(
        schemas["VariantOverrideEnum"]["enum"],
        json!(["up", "down"])
    );
    assert_eq!(schemas["KtEnum"]["enum"], json!(["on", "off"]));
}

#[test]
fn computed_json_value_field_still_warns_with_clickable_file_path() {
    let result = export(&[
        (
            "src/main/java/demo/ComputedEnum.java",
            r#"
package demo;
import com.fasterxml.jackson.annotation.JsonValue;
public enum ComputedEnum {
    A(1);
    @JsonValue
    private final int code;
    ComputedEnum(int arg) { this.code = arg + 1; }
}
"#,
        ),
        (
            "src/main/java/demo/ComputedApi.java",
            r#"
package demo;
import org.springframework.web.bind.annotation.*;
@RestController
public class ComputedApi {
    @GetMapping("/computed")
    public ComputedEnum get() { return null; }
}
"#,
        ),
    ]);

    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    assert!(result.warnings[0].contains("无法可靠解析 demo.ComputedEnum"));
    assert!(result.warnings[0].contains("（src/main/java/demo/ComputedEnum.java）"));
}
