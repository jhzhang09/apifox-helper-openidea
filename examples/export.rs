//! 在命令行检查本地解析结果，不访问 Apifox。
use apifox_helper_openidea::{openapi::Scope, source};
use std::{io::Write, path::Path};
fn collect(
    path: &Path,
    root: &Path,
    classes: &mut Vec<source::Class>,
    settings: &mut apifox_helper_openidea::project::ProjectSettings,
) -> std::io::Result<()> {
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    if source::ignored_path(&relative) {
        return Ok(());
    }
    if path.is_dir() {
        let mut entries = std::fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let p = entry.path();
            if entry.file_type()?.is_symlink() {
                continue;
            }
            if ["target", "build", ".git", "node_modules", "vendor"]
                .contains(&entry.file_name().to_string_lossy().as_ref())
            {
                continue;
            }
            collect(&p, root, classes, settings)?;
        }
    } else if source::production_source(&relative) {
        let text = std::fs::read_to_string(path)?;
        settings.java(&text);
        classes.extend(source::parse(&relative, &text));
    } else if path.file_name().is_some_and(|n| {
        n == "application.yml" || n == "application.yaml" || n == "application.properties"
    }) && let Err(error) = settings.config(&relative, &std::fs::read_to_string(path)?)
    {
        eprintln!("提示：{error}");
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .ok_or("用法：cargo run --example export -- <源码目录>")?;
    let format = apifox_helper_openidea::export::Format::parse(
        &std::env::args().nth(2).unwrap_or_else(|| "json".into()),
    )?;
    let mut classes = Vec::new();
    let path = Path::new(&root);
    let base = if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };
    let mut settings = apifox_helper_openidea::project::ProjectSettings::default();
    collect(path, base, &mut classes, &mut settings)?;
    let export = apifox_helper_openidea::openapi::generate_with_project(
        &classes,
        &Scope::default(),
        "Apifox Helper 示例",
        &settings.finish(),
    );
    for warning in &export.warnings {
        eprintln!("提示：{warning}");
    }
    for notice in &export.notices {
        eprintln!("说明：{notice}");
    }
    if export.endpoints.is_empty() {
        return Err("未识别到接口".into());
    }
    std::io::stdout().write_all(format.render(&export.document)?.as_bytes())?;
    Ok(())
}
