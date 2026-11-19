//! Swagger 说明与 Javadoc 的取值规则；只处理文档，不改变 Spring 参数绑定。
use crate::source::{self, Annotation, Class, Field, Method};
use std::collections::BTreeMap;

pub fn text(annotations: &[Annotation], choices: &[(&str, &str)]) -> Option<String> {
    source::annotation_text(annotations, choices)
}
pub fn model_description(class: &Class) -> String {
    text(
        &class.annotations,
        &[("Schema", "description"), ("ApiModel", "description")],
    )
    .unwrap_or_else(|| source::summary(&class.doc))
}
pub fn model_title(class: &Class) -> Option<String> {
    text(
        &class.annotations,
        &[("Schema", "title"), ("ApiModel", "value")],
    )
}
pub fn field_description(field: &Field) -> Option<String> {
    text(
        &field.annotations,
        &[
            ("Schema", "description"),
            ("ApiModelProperty", "value"),
            ("ApiModelProperty", "notes"),
        ],
    )
    .or_else(|| source::nonempty(&source::summary(&field.doc)))
}
pub fn operation_title(method: &Method) -> String {
    text(
        &method.annotations,
        &[("Operation", "summary"), ("ApiOperation", "value")],
    )
    .or_else(|| source::nonempty(source::summary(&method.doc).lines().next().unwrap_or("")))
    .unwrap_or_else(|| method.name.clone())
}
pub fn operation_description(method: &Method) -> String {
    text(
        &method.annotations,
        &[("Operation", "description"), ("ApiOperation", "notes")],
    )
    .unwrap_or_else(|| source::summary(&method.doc))
}
pub fn controller_title(class: &Class) -> String {
    text(&class.annotations, &[("Tag", "name")])
        .or_else(|| {
            class
                .annotations
                .iter()
                .filter(|a| a.name == "Api" && !a.expressions.contains("tags"))
                .find_map(|a| {
                    a.values("tags")
                        .into_iter()
                        .find_map(|v| source::nonempty(&v))
                })
        })
        .or_else(|| text(&class.annotations, &[("Api", "value")]))
        .or_else(|| source::nonempty(source::summary(&class.doc).lines().next().unwrap_or("")))
        .unwrap_or_else(|| class.name.clone())
}

pub fn operation_tags(class: &Class, method: &Method) -> Vec<String> {
    let tags: Vec<String> = method
        .annotations
        .iter()
        .filter(|a| {
            matches!(a.name.as_str(), "Operation" | "ApiOperation")
                && !a.expressions.contains("tags")
        })
        .flat_map(|a| a.values("tags"))
        .filter_map(|v| source::nonempty(&v))
        .collect();
    if tags.is_empty() {
        vec![controller_title(class)]
    } else {
        tags
    }
}

pub fn swagger_request_body(a: &Annotation) -> bool {
    a.name == "RequestBody"
        && (a.full_name == "io.swagger.v3.oas.annotations.parameters.RequestBody"
            || (!a.full_name.contains('.')
                && (a.args.contains_key("description")
                    || a.args.contains_key("content")
                    || a.args.contains_key("ref"))))
}
pub fn spring_request_body(field: &Field) -> Option<&Annotation> {
    field.annotations.iter().find(|a| {
        a.name == "RequestBody"
            && (a.full_name == "org.springframework.web.bind.annotation.RequestBody"
                || (!a.full_name.contains('.') && !swagger_request_body(a)))
    })
}
fn request_body_annotations<'a>(field: &'a Field, method: &'a Method) -> Vec<&'a Annotation> {
    field
        .annotations
        .iter()
        .chain(method.annotations.iter())
        .filter(|a| swagger_request_body(a))
        .chain(
            method
                .annotations
                .iter()
                .filter(|a| a.name == "Operation")
                .flat_map(|a| a.children("requestBody")),
        )
        .collect()
}
fn method_parameters(method: &Method) -> Vec<&Annotation> {
    method
        .annotations
        .iter()
        .filter(|a| a.name == "Parameter")
        .chain(
            method
                .annotations
                .iter()
                .filter(|a| a.name == "Parameters")
                .flat_map(|a| a.children("value")),
        )
        .chain(
            method
                .annotations
                .iter()
                .filter(|a| a.name == "Operation")
                .flat_map(|a| a.children("parameters")),
        )
        .collect()
}
fn matching_method_parameters<'a>(field: &'a Field, method: &'a Method) -> Vec<&'a Annotation> {
    method_parameters(method)
        .into_iter()
        .filter(|a| {
            a.text("name").is_some_and(|name| {
                name == field.name
                    || field
                        .annotations
                        .iter()
                        .filter(|binding| {
                            [
                                "RequestParam",
                                "PathVariable",
                                "RequestHeader",
                                "CookieValue",
                                "QueryParam",
                                "PathParam",
                                "HeaderParam",
                                "CookieParam",
                            ]
                            .contains(&binding.name.as_str())
                        })
                        .any(|binding| {
                            binding
                                .text("name")
                                .or_else(|| binding.text("value"))
                                .as_deref()
                                == Some(&name)
                        })
            })
        })
        .collect()
}
pub fn parameter_annotation<'a>(field: &'a Field, method: &'a Method) -> Option<&'a Annotation> {
    source::annotation(&field.annotations, "Parameter")
        .or_else(|| matching_method_parameters(field, method).into_iter().next())
}
pub fn parameter_description(field: &Field, method: &Method) -> String {
    text(&field.annotations, &[("Parameter", "description")])
        .or_else(|| {
            matching_method_parameters(field, method)
                .into_iter()
                .find_map(|a| a.text("description"))
        })
        .or_else(|| {
            text(
                &field.annotations,
                &[("ApiParam", "value"), ("Schema", "description")],
            )
        })
        .or_else(|| source::tag(&method.doc, &format!("param {}", field.name)))
        .unwrap_or_default()
}
pub fn request_body_description(field: &Field, method: &Method) -> String {
    request_body_annotations(field, method)
        .into_iter()
        .find_map(|a| a.text("description"))
        .unwrap_or_else(|| parameter_description(field, method))
}

fn response_annotations(annotations: &[Annotation]) -> Vec<&Annotation> {
    annotations
        .iter()
        .filter(|a| a.name == "ApiResponse")
        .chain(
            annotations
                .iter()
                .filter(|a| a.name == "ApiResponses")
                .flat_map(|a| a.children("value")),
        )
        .chain(
            annotations
                .iter()
                .filter(|a| a.name == "Operation")
                .flat_map(|a| a.children("responses")),
        )
        .collect()
}
/// 方法级优先于类级，同一位置 OpenAPI 3 优先于 Swagger 2，空白说明继续回退。
pub fn response_descriptions(class: &Class, method: &Method) -> BTreeMap<String, Option<String>> {
    let mut result = BTreeMap::<String, Option<String>>::new();
    for annotations in [&method.annotations, &class.annotations] {
        let responses = response_annotations(annotations);
        for legacy in [false, true] {
            for a in responses
                .iter()
                .filter(|a| a.args.contains_key("code") == legacy)
            {
                let code = if legacy {
                    a.value("code")
                } else {
                    Some(a.text("responseCode").unwrap_or_else(|| "default".into()))
                };
                let Some(code) = code else {
                    continue;
                };
                // OpenAPI 允许显式状态码、默认响应和 2XX 一类范围，其他值不能生成非法文档。
                if !(code == "default"
                    || (code.len() == 3
                        && matches!(code.as_bytes()[0], b'1'..=b'5')
                        && (code.as_bytes()[1..].iter().all(u8::is_ascii_digit)
                            || code.ends_with("XX"))))
                {
                    continue;
                }
                let description = a.text(if legacy { "message" } else { "description" });
                let entry = result.entry(code).or_default();
                if entry.is_none() {
                    *entry = description;
                }
            }
        }
    }
    result
}
