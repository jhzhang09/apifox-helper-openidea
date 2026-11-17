//! 接口树的模块显示信息，不参与 OpenAPI 内容生成。

/// 读取 POM 根 project 下的 artifactId，排除 parent、依赖、插件和注释中的同名标签。
pub fn pom_artifact_id(xml: &str) -> Option<String> {
    let mut remaining = xml;
    let mut stack: Vec<&str> = Vec::new();
    while let Some(start) = remaining.find('<') {
        remaining = &remaining[start..];
        if remaining.starts_with("<!--") {
            remaining = &remaining[remaining.find("-->")? + 3..];
            continue;
        }
        if remaining.starts_with("<![CDATA[") {
            remaining = &remaining[remaining.find("]]>")? + 3..];
            continue;
        }
        let end = remaining.find('>')?;
        let tag = remaining[1..end].trim();
        remaining = &remaining[end + 1..];
        if tag.starts_with(['?', '!']) {
            continue;
        }
        if let Some(close) = tag.strip_prefix('/') {
            if stack.pop()? != close.trim() {
                return None;
            }
            continue;
        }
        if tag.ends_with('/') {
            continue;
        }
        let name = tag.split_whitespace().next()?;
        if name.rsplit(':').next() == Some("artifactId")
            && stack.len() == 1
            && stack[0].rsplit(':').next() == Some("project")
        {
            let value = remaining.split('<').next()?.trim();
            // 未求值的 Maven 属性不冒充模块名称，退回目录名称。
            return (!value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
            .then(|| value.to_string());
        }
        stack.push(name);
    }
    None
}
