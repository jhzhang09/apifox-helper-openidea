//! 注解与 Javadoc 冲突、空白值、命名空间和嵌套注解的回归用例。
use apifox_helper_openidea::{
    openapi::{Export, Scope, generate},
    source,
};
use serde_json::{Value, json};
fn export(code: &str) -> Export {
    generate(
        &source::parse("Api.java", code),
        &Scope::default(),
        "说明回归",
    )
}
fn op(result: &Export, path: &str) -> Value {
    result.document["paths"][path]["post"].clone()
}

#[test]
fn model_and_field_descriptions_use_modern_then_legacy_then_javadoc() {
    for (modern, legacy, expected) in [
        ("新说明", "旧说明", "新说明"),
        (" ", "旧说明", "旧说明"),
        ("", "\n\t", "文档说明"),
    ] {
        let code = format!(
            r#"@RestController class Api {{ @PostMapping("/x") public Payload x(){{return null;}} }}
/** 文档说明 */ @Schema(description={modern:?}) @ApiModel(description={legacy:?}) class Payload {{
 /** 文档说明 */ @Schema(description={modern:?}) @ApiModelProperty(value={legacy:?}) private String field;
}}"#
        );
        let r = export(&code);
        assert!(r.warnings.is_empty());
        let schema = &r.document["components"]["schemas"]["Payload"];
        assert_eq!(schema["description"], expected);
        assert_eq!(schema["properties"]["field"]["description"], expected);
    }
}
#[test]
fn model_titles_use_nonempty_swagger_titles_without_renaming_components() {
    let r = export(
        r#"@RestController class Api { @PostMapping("/x") public Payload x(){return null;} }
@Schema(title="新标题") @ApiModel(value="旧标题") class Payload {private String x;}
"#,
    );
    let m = &r.document["components"]["schemas"]["Payload"];
    assert_eq!(m["title"], "新标题");
    let r = export(
        r#"@RestController class Api { @PostMapping("/x") public Payload x(){return null;} }
@Schema(title="  ") @ApiModel(value="旧标题") class Payload {private String x;}
"#,
    );
    assert_eq!(
        r.document["components"]["schemas"]["Payload"]["title"],
        "旧标题"
    );
}
#[test]
fn operation_summary_description_and_controller_titles_skip_blank_annotations() {
    let r = export(
        r#"/** 控制器文档 */ @RestController @Tag(name=" ") @Api(tags={"旧分组"},value="旧名称") class Api {
 /** 方法文档 */ @Operation(summary=" ",description="") @ApiOperation(value="旧接口名",notes="旧详细说明")
 @PostMapping("/legacy") public void legacy(){}
 /** 方法文档 */ @Operation(summary="新接口名",description="新详细说明") @ApiOperation(value="旧接口名",notes="旧详细说明")
 @PostMapping("/modern") public void modern(){}
 /** 文档接口名
  * 第二行说明
  */ @Operation(summary="",description="\n") @ApiOperation(value=" ",notes=" ") @PostMapping("/doc") public void docs(){}
 @Operation(summary=" ") @PostMapping("/empty") public void fallbackName(){}
}"#,
    );
    assert_eq!(op(&r, "/legacy")["summary"], "旧接口名");
    assert_eq!(op(&r, "/legacy")["description"], "旧详细说明");
    assert_eq!(op(&r, "/legacy")["tags"], json!(["旧分组"]));
    assert_eq!(op(&r, "/modern")["summary"], "新接口名");
    assert_eq!(op(&r, "/modern")["description"], "新详细说明");
    assert_eq!(op(&r, "/doc")["summary"], "文档接口名");
    assert_eq!(op(&r, "/doc")["description"], "文档接口名\n第二行说明");
    assert_eq!(op(&r, "/empty")["summary"], "fallbackName");
}
#[test]
fn parameter_annotations_override_javadoc_without_losing_multiline_fallbacks() {
    let r = export(
        r#"@RestController class Api {
 /** 提交
  * @param a 参数文档
  * @param b 第二参数文档
  * @param    c   第三参数文档
  *   第二行解释
  * @return 无返回值
  */ @PostMapping("/x") public void x(
 @Parameter(description="新参数") @ApiParam(value="旧参数") @RequestParam String a,
 @Parameter(description=" ") @ApiParam(value="旧参数") @RequestParam String b,
 @Parameter(description="") @ApiParam(value="\t") @RequestParam String c){}
}"#,
    );
    let p = op(&r, "/x")["parameters"].as_array().unwrap().clone();
    assert_eq!(p[0]["description"], "新参数");
    assert_eq!(p[1]["description"], "旧参数");
    assert_eq!(p[2]["description"], "第三参数文档\n第二行解释");
}
#[test]
fn method_level_and_operation_embedded_parameters_match_by_parameter_name() {
    let r = export(
        r#"@RestController class Api {
 /** 提交
  * @param a 文档 A
  * @param b 文档 B
  */ @Parameters({@Parameter(name="a",description="方法参数 A"),@Parameter(name="b",description=" ")})
 @Operation(parameters={@Parameter(name="b",description="内嵌参数 B")}) @PostMapping("/x")
 public void x(@RequestParam String a,@RequestParam String b){}
}"#,
    );
    let p = op(&r, "/x")["parameters"].as_array().unwrap().clone();
    assert_eq!(p[0]["description"], "方法参数 A");
    assert_eq!(p[1]["description"], "内嵌参数 B");
}
#[test]
fn request_body_namespace_description_and_binding_requiredness_stay_separate() {
    let r = export(
        r#"import org.springframework.web.bind.annotation.RequestBody;
@RestController class Api {
 /** 提交
  * @param request 请求参数文档
  */ @PostMapping("/x") public Payload x(
 @RequestBody(required=false) @io.swagger.v3.oas.annotations.parameters.RequestBody(description="请求体注解",required=true)
 @Parameter(description="参数注解") Payload request){return request;}
}
/** 模型文档 */ class Payload {/** 字段文档 */ private String reason;}
"#,
    );
    let operation = op(&r, "/x");
    assert_eq!(operation["requestBody"]["description"], "请求体注解");
    assert_eq!(operation["requestBody"]["required"], false);
    assert!(operation["parameters"].as_array().unwrap().is_empty());
    let m = &r.document["components"]["schemas"]["Payload"];
    assert_eq!(m["description"], "模型文档");
    assert_eq!(m["properties"]["reason"]["description"], "字段文档");
}
#[test]
fn request_body_parameter_method_embedded_and_javadoc_fallbacks() {
    let r = export(
        r#"import io.swagger.v3.oas.annotations.parameters.RequestBody;
@RestController class Api {
 /** 提交
  * @param request 请求体文档
  */ @RequestBody(description="方法请求体")
 @Operation(requestBody=@RequestBody(description="内嵌请求体")) @PostMapping("/method")
 public void method(@org.springframework.web.bind.annotation.RequestBody @RequestBody(description=" ") Payload request){}
 /** 提交
  * @param request 请求体文档
  */ @RequestBody(description=" ")
 @Operation(requestBody=@RequestBody(description="内嵌请求体")) @PostMapping("/embedded")
 public void embedded(@org.springframework.web.bind.annotation.RequestBody Payload request){}
 /** 提交
  * @param request 请求体文档
  */ @RequestBody(description=" ")
 @Operation(requestBody=@RequestBody(description="")) @PostMapping("/doc")
 public void docs(@org.springframework.web.bind.annotation.RequestBody @ApiParam(value=" ") Payload request){}
} class Payload{private String reason;}
"#,
    );
    assert_eq!(
        op(&r, "/method")["requestBody"]["description"],
        "方法请求体"
    );
    assert_eq!(
        op(&r, "/embedded")["requestBody"]["description"],
        "内嵌请求体"
    );
    assert_eq!(op(&r, "/doc")["requestBody"]["description"], "请求体文档");
}
#[test]
fn swagger_request_body_alone_is_not_a_spring_binding_annotation() {
    for import in [
        "io.swagger.v3.oas.annotations.parameters.RequestBody",
        "io.swagger.v3.oas.annotations.parameters.*",
    ] {
        let r = export(&format!(
            r#"import {import};
@RestController class Api{{ @PostMapping("/x") public void x(@RequestBody @RequestParam String value){{}} }}"#
        ));
        assert!(op(&r, "/x").get("requestBody").is_none());
        assert_eq!(op(&r, "/x")["parameters"][0]["name"], "value");
    }
}
#[test]
fn response_descriptions_support_repeatable_container_and_operation_annotations() {
    let r = export(
        r#"@RestController class Api {
 /** 提交
  * @return 返回文档
  */ @ApiResponse(responseCode="200",description=" ")
 @ApiResponses({@ApiResponse(responseCode="400",description="参数错误"),@ApiResponse(responseCode="404",description="资源缺失")})
 @Operation(responses={@ApiResponse(responseCode="200",description="内嵌成功"),@ApiResponse(description="默认响应")})
 @PostMapping("/x") public Payload x(){return null;}
} class Payload{private String reason;}
"#,
    );
    let resp = &op(&r, "/x")["responses"];
    assert_eq!(resp["200"]["description"], "内嵌成功");
    assert_eq!(resp["400"]["description"], "参数错误");
    assert_eq!(resp["404"]["description"], "资源缺失");
    assert_eq!(resp["default"]["description"], "默认响应");
    assert_eq!(
        resp["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Payload"
    );
    assert!(resp["400"].get("content").is_none());
}
#[test]
fn legacy_responses_and_blank_modern_responses_fall_back_without_losing_return_schema() {
    let r = export(
        r#"@RestController @ApiResponses({@ApiResponse(responseCode="200",description="类级说明")}) class Api {
 /** 提交
  * @return 文档返回说明
  */ @io.swagger.v3.oas.annotations.responses.ApiResponse(responseCode="200",description=" ")
 @io.swagger.annotations.ApiResponses({@io.swagger.annotations.ApiResponse(code=200,message="旧成功"),@io.swagger.annotations.ApiResponse(code=403,message="旧禁止")})
 @PostMapping("/x") public R<Payload> x(){return null;}
} class R<T>{private T data;} class Payload{private String reason;}
"#,
    );
    let resp = op(&r, "/x")["responses"].clone();
    assert_eq!(resp["200"]["description"], "旧成功");
    assert_eq!(resp["403"]["description"], "旧禁止");
    assert_eq!(
        resp["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/RPayload"
    );
}
#[test]
fn response_status_matches_description_and_empty_annotations_keep_javadoc() {
    let r = export(
        r#"@RestController class Api {
 /** 创建
  * @return 创建结果
  * 以及服务器时间
  */ @ResponseStatus(HttpStatus.CREATED)
 @ApiResponse(responseCode="201",description=" ") @PostMapping("/created") public String created(){return "";}
 @ApiResponse(responseCode="not-a-status",description="无效状态") @PostMapping("/empty") public void empty(){}
}"#,
    );
    let r201 = op(&r, "/created")["responses"].clone();
    assert_eq!(r201["201"]["description"], "创建结果\n以及服务器时间");
    assert_eq!(op(&r, "/empty")["responses"]["200"]["description"], "成功");
    assert!(op(&r, "/empty")["responses"].get("not-a-status").is_none());
}
#[test]
fn nested_annotations_never_leak_text_into_the_parent_or_sibling() {
    let classes = source::parse(
        "Api.java",
        r#"@RestController class Api{
 @Operation(summary="标题",requestBody=@io.swagger.v3.oas.annotations.parameters.RequestBody(description="请求体",content=@Content(schema=@Schema(description="字段"))),responses={@ApiResponse(responseCode="200",description="响应")})
 @PostMapping("/x") public void x(@RequestBody String request){}
}"#,
    );
    let operation = source::annotation(&classes[0].methods[0].annotations, "Operation").unwrap();
    assert_eq!(operation.text("summary").as_deref(), Some("标题"));
    assert_eq!(operation.text("description"), None);
    assert_eq!(
        operation.children("requestBody")[0]
            .text("description")
            .as_deref(),
        Some("请求体")
    );
    assert_eq!(
        operation.children("responses")[0]
            .text("description")
            .as_deref(),
        Some("响应")
    );
    assert_eq!(
        operation.children("requestBody")[0].children("content")[0].children("schema")[0]
            .text("description")
            .as_deref(),
        Some("字段")
    );
}

#[test]
fn repeated_response_and_parameter_aliases_respect_nonempty_priority() {
    let r = export(
        r#"@RestController class Api{
 @ApiResponse(responseCode="200",description=" ")
 @ApiResponse(responseCode="200",description="新响应")
 @io.swagger.annotations.ApiResponse(code=200,message="旧响应")
 @Parameter(name="external_name",description="外部参数说明")
 @PostMapping("/x") public String x(@RequestParam("external_name") String internalName){return "";}
}"#,
    );
    assert_eq!(op(&r, "/x")["responses"]["200"]["description"], "新响应");
    assert_eq!(op(&r, "/x")["parameters"][0]["description"], "外部参数说明");
}
#[test]
fn blank_duplicate_javadoc_tags_and_field_notes_keep_valid_fallbacks() {
    assert_eq!(
        source::tag("@return  \n@param x 其他说明\n@return 有效说明", "return").as_deref(),
        Some("有效说明")
    );
    let r = export(
        r#"@RestController class Api{ @PostMapping("/x") public Payload x(){return null;} }
/** 模型文档 */ @Schema(title=" ") @ApiModel(value=" ") class Payload{
 /** 字段文档 */ @Schema(description=" ") @ApiModelProperty(value="",notes="旧字段补充说明") private String name;
}"#,
    );
    let p = &r.document["components"]["schemas"]["Payload"];
    assert_eq!(p["title"], "Payload");
    assert_eq!(p["description"], "模型文档");
    assert_eq!(p["properties"]["name"]["description"], "旧字段补充说明");
}
