//! 将 OpenAPI 的 JSON 数据模型输出为 JSON 或 YAML，不改变字段类型及引用。
use openidea_plugin::Error;
use serde_json::Value;

pub enum Format {
    Json,
    Yaml,
}
impl Format {
    pub fn parse(name: &str) -> Result<Self, Error> {
        match name {
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            _ => Err("不支持的导出格式".into()),
        }
    }
    pub fn path(&self) -> &'static str {
        match self {
            Self::Json => "apifox-openapi.json",
            Self::Yaml => "apifox-openapi.yaml",
        }
    }
    pub fn render(&self, document: &Value) -> Result<String, Error> {
        match self {
            Self::Json => {
                serde_json::to_string_pretty(document).map_err(|e| Error::Invalid(e.to_string()))
            }
            Self::Yaml => {
                let mut out = String::new();
                yaml(document, 0, &mut out);
                Ok(out)
            }
        }
    }
}

fn scalar(value: &Value) -> bool {
    match value {
        Value::Object(v) => v.is_empty(),
        Value::Array(v) => v.is_empty(),
        _ => true,
    }
}
fn atom(value: &Value) -> String {
    // YAML 会规范化这些 Unicode 换行符，必须转义才能保留原始字符串。
    value
        .to_string()
        .replace('\u{85}', "\\u0085")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}
fn yaml(value: &Value, indent: usize, out: &mut String) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                out.push_str(&" ".repeat(indent));
                // 所有键和值中的字符串都加 JSON 双引号，避免日期、数字、冒号等歧义。
                out.push_str(&atom(&Value::String(key.clone())));
                out.push(':');
                child(value, indent, out);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for value in items {
                out.push_str(&" ".repeat(indent));
                out.push('-');
                child(value, indent, out);
            }
        }
        _ => {
            out.push_str(&" ".repeat(indent));
            out.push_str(&atom(value));
            out.push('\n');
        }
    }
}
fn child(value: &Value, indent: usize, out: &mut String) {
    if scalar(value) {
        out.push(' ');
        out.push_str(&atom(value));
        out.push('\n');
    } else {
        out.push('\n');
        yaml(value, indent + 2, out);
    }
}
