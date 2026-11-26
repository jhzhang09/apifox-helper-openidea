//! 按需从本地 Maven / Gradle 依赖仓库解析项目缺失的外部类型（`-sources.jar` 优先，`.class` 字节码兜底）。
use crate::{
    classfile::{self, DEFAULT_MAX_ENTRY_BYTES, ZipIndex},
    openapi::Scope,
    schema::{ClassIndex, ResolveOutcome, Schemas, Type},
    source::{self, Class, has},
};
use openidea_plugin::{Error, host};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const MAX_JAR_BYTES: u64 = 15 * 1024 * 1024;
const MAX_TOTAL_JAR_BYTES: usize = 32 * 1024 * 1024;
const MAX_JARS: usize = 64;
const MAX_PROBED_JARS: usize = 128;
const MAX_CLASSES: usize = 512;
const MAX_ROUNDS: usize = 6;
const MAX_POM_FILES: u32 = 200;

thread_local! {
    static FAILED_MVN_FINGERPRINTS: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

const DEFAULT_REPO_ROOTS: &[&str] = &["~/.m2/repository", "~/.gradle/caches/modules-2/files-2.1"];

#[derive(Clone, Debug, Default)]
pub struct ExternalResolution {
    pub classes: Vec<Class>,
    pub resolved_count: usize,
    pub jar_count: usize,
    pub javap_count: usize,
}

pub fn resolve_external_classes(
    project: &host::Project,
    root: &str,
    classes: &[Class],
    scope: &Scope,
) -> ExternalResolution {
    resolve_external_classes_with_task(project, root, classes, scope, None).unwrap_or_default()
}

pub fn resolve_external_classes_with_task(
    project: &host::Project,
    _root: &str,
    classes: &[Class],
    scope: &Scope,
    task: Option<&host::Task>,
) -> Result<ExternalResolution, Error> {
    resolve_external_classes_impl(project, classes, scope, DEFAULT_REPO_ROOTS, task)
}

pub fn resolve_external_classes_with_roots(
    project: &host::Project,
    _root: &str,
    classes: &[Class],
    scope: &Scope,
    repo_roots: &[&str],
) -> ExternalResolution {
    resolve_external_classes_impl(project, classes, scope, repo_roots, None).unwrap_or_default()
}

fn check_cancelled(task: Option<&host::Task>) -> Result<(), Error> {
    if task.is_some_and(|t| t.is_cancelled()) {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn resolve_external_classes_impl(
    project: &host::Project,
    classes: &[Class],
    scope: &Scope,
    repo_roots: &[&str],
    task: Option<&host::Task>,
) -> Result<ExternalResolution, Error> {
    check_cancelled(task)?;
    let mut all_classes: Vec<Class> = classes.to_vec();
    let mut known_fqns: HashSet<String> = all_classes.iter().map(Class::qualified).collect();
    let mut resolved_classes: Vec<Class> = Vec::new();
    let mut attempted_fqns: HashSet<String> = HashSet::new();

    let first_round = collect_missing_fqns(&all_classes, scope, &mut attempted_fqns);
    if first_round.all.is_empty() {
        return Ok(ExternalResolution::default());
    }

    if let Some(t) = task {
        t.report(0, None, Some("正在分析外部依赖类型..."));
    }

    let active_roots: Vec<String> = repo_roots
        .iter()
        .filter(|r| host::fs::stat(r).is_ok())
        .map(|r| r.trim_end_matches(['/', '\\']).to_string())
        .collect();

    let mut deps = collect_project_deps(project, &active_roots);
    let mut jar_count = 0usize;

    if !active_roots.is_empty() {
        let mut cache = JarCache::new(active_roots);
        let mut pending = first_round.all;

        for _round in 0..MAX_ROUNDS {
            check_cancelled(task)?;
            if pending.is_empty() || resolved_classes.len() >= MAX_CLASSES {
                break;
            }
            let total = pending.len();
            let mut newly_added = false;
            for (i, fqn) in pending.into_iter().enumerate() {
                check_cancelled(task)?;
                if resolved_classes.len() >= MAX_CLASSES {
                    break;
                }
                if known_fqns.contains(&fqn) {
                    continue;
                }
                if let Some(t) = task
                    && (i == 0 || i % 4 == 0 || i + 1 == total)
                {
                    t.report(
                        (i + 1) as u32,
                        Some(total as u32),
                        Some(&format!("解析外部类型：{fqn}")),
                    );
                }
                let parsed = cache.resolve_fqn(&fqn, &mut deps, task)?;
                if !parsed.is_empty() {
                    for cls in parsed {
                        if known_fqns.insert(cls.qualified()) {
                            all_classes.push(cls.clone());
                            resolved_classes.push(cls);
                            newly_added = true;
                            if resolved_classes.len() >= MAX_CLASSES {
                                break;
                            }
                        }
                    }
                }
            }
            if !newly_added {
                break;
            }
            pending = collect_missing_fqns(&all_classes, scope, &mut attempted_fqns).all;
        }
        jar_count = cache.loaded_jar_count();
    }

    let mut javap_count = 0usize;
    if deps.has_pom && resolved_classes.len() < MAX_CLASSES {
        let mut javap_attempted = HashSet::new();
        let remaining = collect_missing_fqns(&all_classes, scope, &mut javap_attempted);
        let has_explicit_missing = remaining.explicit.iter().any(|f| !known_fqns.contains(f));
        if !remaining.all.is_empty() && (jar_count == 0 || has_explicit_missing) {
            javap_count = resolve_via_mvn_javap(
                project,
                &deps,
                scope,
                &mut all_classes,
                &mut known_fqns,
                &mut resolved_classes,
                remaining.all,
                &mut javap_attempted,
                task,
            )?;
        }
    }

    Ok(ExternalResolution {
        resolved_count: resolved_classes.len(),
        classes: resolved_classes,
        jar_count,
        javap_count,
    })
}

fn is_builtin_or_jdk_type(name: &str) -> bool {
    let simple = name.rsplit('.').next().unwrap_or(name);
    matches!(
        simple,
        "byte"
            | "short"
            | "int"
            | "long"
            | "float"
            | "double"
            | "boolean"
            | "char"
            | "void"
            | "Void"
            | "Unit"
            | "Object"
            | "Any"
            | "String"
            | "CharSequence"
            | "Integer"
            | "Int"
            | "Long"
            | "Double"
            | "Float"
            | "Boolean"
            | "Short"
            | "Byte"
            | "Character"
            | "Char"
            | "BigDecimal"
            | "BigInteger"
            | "Number"
            | "List"
            | "Set"
            | "Collection"
            | "Iterable"
            | "ArrayList"
            | "LinkedList"
            | "HashSet"
            | "Map"
            | "HashMap"
            | "LinkedHashMap"
            | "TreeMap"
            | "SortedMap"
            | "ConcurrentHashMap"
            | "AbstractMap"
            | "MutableList"
            | "MutableSet"
            | "MutableMap"
            | "Sequence"
            | "Array"
            | "Optional"
            | "ResponseEntity"
            | "HttpEntity"
            | "Mono"
            | "Flux"
            | "CompletionStage"
            | "CompletableFuture"
            | "DeferredResult"
            | "Callable"
            | "MultipartFile"
            | "File"
            | "InputStream"
            | "LocalDate"
            | "LocalDateTime"
            | "OffsetDateTime"
            | "ZonedDateTime"
            | "LocalTime"
            | "OffsetTime"
            | "Date"
            | "Instant"
            | "Timestamp"
            | "UUID"
            | "HttpServletRequest"
            | "HttpServletResponse"
            | "HttpSession"
            | "ServletRequest"
            | "ServletResponse"
            | "NativeWebRequest"
            | "ServerWebExchange"
            | "ServerHttpRequest"
            | "ServerHttpResponse"
            | "RedirectAttributes"
            | "SessionStatus"
            | "UriComponentsBuilder"
            | "ServletUriComponentsBuilder"
            | "Principal"
            | "Authentication"
            | "BindingResult"
            | "Errors"
            | "Model"
            | "ModelMap"
            | "WebRequest"
            | "Continuation"
            | "Serializable"
            | "Comparable"
            | "Cloneable"
            | "AutoCloseable"
            | "Enum"
            | "Record"
            | "?"
            | "*"
            | ""
    )
}

fn is_ignored_external_package(fqn: &str) -> bool {
    [
        "java.",
        "javax.",
        "jakarta.",
        "sun.",
        "com.sun.",
        "jdk.",
        "kotlin.",
        "kotlinx.",
        "scala.",
        "reactor.core.",
        "org.reactivestreams.",
        "org.springframework.http.codec.multipart.",
        "org.springframework.web.multipart.",
        "org.springframework.web.server.",
        "org.springframework.data.util.Pair",
        "org.springframework.core.io.",
        "org.springframework.web.servlet.mvc.method.annotation.SseEmitter",
        "cn.hutool.core.lang.tree.Tree",
        "cn.hutool.core.lang.Dict",
        "cn.hutool.json.",
        "com.alibaba.fastjson.",
        "com.alibaba.fastjson2.",
        "org.json.",
        "com.fasterxml.jackson.databind.JsonNode",
        "com.fasterxml.jackson.databind.node.",
        "com.google.gson.Json",
    ]
    .iter()
    .any(|prefix| fqn.starts_with(prefix))
}

struct MissingFqns {
    all: Vec<String>,
    explicit: HashSet<String>,
}

fn collect_missing_fqns(
    classes: &[Class],
    scope: &Scope,
    attempted_fqns: &mut HashSet<String>,
) -> MissingFqns {
    let index = ClassIndex::new(classes);
    let mut seen_classes = HashSet::new();
    let mut controller_ancestors = HashSet::new();
    let mut expanded_ctrl_methods = HashSet::new();
    let mut class_queue: Vec<&Class> = Vec::new();
    let mut type_queue: Vec<(Type, &Class, bool)> = Vec::new();
    let mut missing = BTreeSet::new();
    let mut explicit = HashSet::new();

    for c in classes {
        if c.is_external
            || !scope.accepts_class(c)
            || !["RestController", "Controller", "FeignClient", "Path"]
                .iter()
                .any(|a| has(&c.annotations, a))
        {
            continue;
        }
        controller_ancestors.insert(c.qualified());
        expanded_ctrl_methods.insert(c.qualified());
        if seen_classes.insert(c.qualified()) {
            class_queue.push(c);
        }
        for m in &c.methods {
            if !scope.accepts(c, m) {
                continue;
            }
            type_queue.push((Type::parse(&m.ty), c, false));
            for p in &m.params {
                type_queue.push((Type::parse(&p.ty), c, false));
            }
        }
    }

    while !class_queue.is_empty() || !type_queue.is_empty() {
        while let Some(cls) = class_queue.pop() {
            let is_ctrl = controller_ancestors.contains(&cls.qualified());
            if is_ctrl && expanded_ctrl_methods.insert(cls.qualified()) {
                for m in &cls.methods {
                    if crate::openapi::is_endpoint_method(m)
                        || crate::source::annotation(&m.annotations, "Path").is_some()
                    {
                        type_queue.push((Type::parse(&m.ty), cls, false));
                        for p in &m.params {
                            type_queue.push((Type::parse(&p.ty), cls, false));
                        }
                    }
                }
            }
            for base in &cls.bases {
                type_queue.push((Type::parse(base), cls, is_ctrl));
            }
            if is_ctrl || cls.is_interface {
                for iface in &cls.interfaces {
                    type_queue.push((Type::parse(iface), cls, is_ctrl));
                }
            }
            for f in &cls.fields {
                type_queue.push((Type::parse(&f.ty), cls, false));
            }
        }
        while let Some((ty, ctx, from_ctrl_base)) = type_queue.pop() {
            collect_type_candidates(
                &ty,
                ctx,
                from_ctrl_base,
                &index,
                &mut seen_classes,
                &mut controller_ancestors,
                &mut class_queue,
                &mut missing,
                &mut explicit,
                attempted_fqns,
            );
        }
    }

    MissingFqns {
        all: missing.into_iter().collect(),
        explicit,
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_type_candidates<'a>(
    ty: &Type,
    ctx: &'a Class,
    from_ctrl_base: bool,
    index: &ClassIndex<'a>,
    seen_classes: &mut HashSet<String>,
    controller_ancestors: &mut HashSet<String>,
    class_queue: &mut Vec<&'a Class>,
    missing: &mut BTreeSet<String>,
    explicit: &mut HashSet<String>,
    attempted_fqns: &mut HashSet<String>,
) {
    for arg in &ty.args {
        collect_type_candidates(
            arg,
            ctx,
            from_ctrl_base,
            index,
            seen_classes,
            controller_ancestors,
            class_queue,
            missing,
            explicit,
            attempted_fqns,
        );
    }
    let name = ty.name.trim();
    if name.is_empty()
        || is_builtin_or_jdk_type(name)
        || ctx.generics.iter().any(|g| g == name)
        || (name.len() == 1 && name.chars().all(|c| c.is_ascii_uppercase()))
    {
        return;
    }
    if let ResolveOutcome::Found(found) = index.resolve(name, ctx, true) {
        let q = found.qualified();
        let added_ctrl = from_ctrl_base && controller_ancestors.insert(q.clone());
        if seen_classes.insert(q) || added_ctrl {
            class_queue.push(found);
        }
        return;
    }

    let ext = Schemas::external_name(name, ctx);
    let mut candidates = Vec::new();
    let is_qualified_pkg =
        ext.contains('.') && !ext.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    if is_qualified_pkg {
        candidates.push((ext, true));
    } else {
        if !ctx.package.is_empty() {
            candidates.push((format!("{}.{}", ctx.package, ext), false));
        }
        for imp in &ctx.imports {
            if let Some(pkg) = imp.strip_suffix(".*") {
                candidates.push((format!("{pkg}.{ext}"), false));
            }
        }
    }
    for (fqn, is_explicit) in candidates {
        if fqn.contains('.')
            && !is_ignored_external_package(&fqn)
            && !index.contains_qualified(&fqn)
            && attempted_fqns.insert(fqn.clone())
        {
            if is_explicit {
                explicit.insert(fqn.clone());
            }
            missing.insert(fqn);
        }
    }
}

#[derive(Clone, Debug, Default)]
struct PomData {
    self_coord: Option<(String, String)>,
    parent: Option<(String, String, String)>,
    properties: BTreeMap<String, String>,
    managed_deps: Vec<(String, String, Option<String>, Option<String>)>,
    deps: Vec<(String, String, Option<String>)>,
}

#[derive(Default)]
struct ProjectDeps {
    preferred_versions: HashMap<(String, String), String>,
    declared_deps: Vec<(String, String, Option<String>)>,
    workspace_modules: HashSet<(String, String)>,
    loaded_parent_poms: HashSet<(String, String, String)>,
    has_pom: bool,
    pom_fingerprint: u64,
}

fn strip_xml_comments(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start + 4..].find("-->") {
            rest = &rest[start + 4 + end + 3..];
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn extract_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim())
}

fn extract_all_blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let inner_start = start + open.len();
        let Some(end) = rest[inner_start..].find(&close) else {
            break;
        };
        out.push(&rest[inner_start..inner_start + end]);
        rest = &rest[inner_start + end + close.len()..];
    }
    out
}

fn interpolate_props(raw: &str, props: &BTreeMap<String, String>) -> String {
    let mut cur = raw.trim().to_string();
    for _ in 0..8 {
        let Some(start) = cur.find("${") else {
            break;
        };
        let Some(end) = cur[start + 2..].find('}') else {
            break;
        };
        let key = &cur[start + 2..start + 2 + end];
        let Some(val) = props.get(key) else {
            break;
        };
        cur = format!("{}{val}{}", &cur[..start], &cur[start + 2 + end + 1..]);
    }
    cur
}

fn parse_pom(xml: &str) -> PomData {
    let cleaned = strip_xml_comments(xml);
    let mut data = PomData::default();
    if let Some(parent_xml) = extract_tag(&cleaned, "parent")
        && let (Some(g), Some(a), Some(v)) = (
            extract_tag(parent_xml, "groupId"),
            extract_tag(parent_xml, "artifactId"),
            extract_tag(parent_xml, "version"),
        )
    {
        data.parent = Some((g.to_string(), a.to_string(), v.to_string()));
        data.properties
            .entry("project.parent.version".into())
            .or_insert_with(|| v.to_string());
    }
    let mut top_only = cleaned.clone();
    for block in [
        "parent",
        "dependencies",
        "dependencyManagement",
        "build",
        "reporting",
        "profiles",
        "properties",
    ] {
        let open = format!("<{block}>");
        let close = format!("</{block}>");
        while let Some(s) = top_only.find(&open) {
            if let Some(rel_e) = top_only[s..].find(&close) {
                top_only = format!("{}{}", &top_only[..s], &top_only[s + rel_e + close.len()..]);
            } else {
                break;
            }
        }
    }
    if let Some(ver) = extract_tag(&top_only, "version")
        && !ver.contains('<')
    {
        data.properties
            .entry("project.version".into())
            .or_insert_with(|| ver.to_string());
    } else if let Some((_, _, v)) = &data.parent {
        data.properties
            .entry("project.version".into())
            .or_insert_with(|| v.clone());
    }
    if let Some(grp) = extract_tag(&top_only, "groupId")
        && !grp.contains('<')
    {
        data.properties
            .entry("project.groupId".into())
            .or_insert_with(|| grp.to_string());
    } else if let Some((g, _, _)) = &data.parent {
        data.properties
            .entry("project.groupId".into())
            .or_insert_with(|| g.clone());
    }
    let self_group = data.properties.get("project.groupId").cloned();
    if let (Some(sg), Some(sa)) = (self_group, extract_tag(&top_only, "artifactId"))
        && !sa.contains('<')
    {
        data.self_coord = Some((sg, sa.to_string()));
    }
    if let Some(props_xml) = extract_tag(&cleaned, "properties") {
        let mut rest = props_xml;
        while let Some(open) = rest.find('<') {
            let Some(close_angle) = rest[open + 1..].find('>') else {
                break;
            };
            let tag = rest[open + 1..open + 1 + close_angle].trim();
            if tag.starts_with('/') || tag.ends_with('/') || tag.contains(' ') {
                rest = &rest[open + 1 + close_angle + 1..];
                continue;
            }
            let close_tag = format!("</{tag}>");
            let val_start = open + 1 + close_angle + 1;
            if let Some(val_end) = rest[val_start..].find(&close_tag) {
                let val = rest[val_start..val_start + val_end].trim();
                data.properties.insert(tag.to_string(), val.to_string());
                rest = &rest[val_start + val_end + close_tag.len()..];
            } else {
                rest = &rest[val_start..];
            }
        }
    }

    if let Some(dm_xml) = extract_tag(&cleaned, "dependencyManagement") {
        for dep_xml in extract_all_blocks(dm_xml, "dependency") {
            if let (Some(g), Some(a)) = (
                extract_tag(dep_xml, "groupId"),
                extract_tag(dep_xml, "artifactId"),
            ) {
                let v = extract_tag(dep_xml, "version").map(str::to_string);
                let s = extract_tag(dep_xml, "scope").map(str::to_string);
                data.managed_deps.push((g.to_string(), a.to_string(), v, s));
            }
        }
    }

    // 移除 dependencyManagement 块后再提取直接 dependencies
    let direct_xml = if let (Some(s), Some(e)) = (
        cleaned.find("<dependencyManagement>"),
        cleaned.find("</dependencyManagement>"),
    ) && e > s
    {
        format!(
            "{}{}",
            &cleaned[..s],
            &cleaned[e + "</dependencyManagement>".len()..]
        )
    } else {
        cleaned.clone()
    };
    for deps_block in extract_all_blocks(&direct_xml, "dependencies") {
        for dep_xml in extract_all_blocks(deps_block, "dependency") {
            if let (Some(g), Some(a)) = (
                extract_tag(dep_xml, "groupId"),
                extract_tag(dep_xml, "artifactId"),
            ) {
                let v = extract_tag(dep_xml, "version").map(str::to_string);
                data.deps.push((g.to_string(), a.to_string(), v));
            }
        }
    }
    data
}

fn fnv1a_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

struct WorkspacePom {
    depth: usize,
    path: String,
    data: PomData,
}

fn load_repo_pom_cached<'a>(
    repo_roots: &[String],
    group: &str,
    artifact: &str,
    version: &str,
    cache: &'a mut HashMap<(String, String, String), Option<PomData>>,
    loaded_parent_poms: &mut HashSet<(String, String, String)>,
) -> Option<&'a PomData> {
    let key = (group.to_string(), artifact.to_string(), version.to_string());
    if !cache.contains_key(&key) {
        loaded_parent_poms.insert(key.clone());
        let parsed = read_repo_pom(repo_roots, group, artifact, version).map(|t| parse_pom(&t));
        cache.insert(key.clone(), parsed);
    }
    cache.get(&key).and_then(|opt| opt.as_ref())
}

fn collect_pom_chain(
    start_idx: usize,
    poms: &[WorkspacePom],
    workspace_by_coord: &HashMap<(String, String), usize>,
    repo_roots: &[String],
    repo_cache: &mut HashMap<(String, String, String), Option<PomData>>,
    loaded_parent_poms: &mut HashSet<(String, String, String)>,
) -> Vec<PomData> {
    let mut chain = Vec::new();
    let mut seen_ws = HashSet::new();
    let mut seen_repo = HashSet::new();
    seen_ws.insert(start_idx);
    chain.push(poms[start_idx].data.clone());
    let mut next_parent = poms[start_idx].data.parent.clone();

    for _ in 0..6 {
        let Some((pg, pa, pv)) = next_parent else {
            break;
        };
        if let Some(&ws_idx) = workspace_by_coord.get(&(pg.clone(), pa.clone()))
            && seen_ws.insert(ws_idx)
        {
            chain.push(poms[ws_idx].data.clone());
            next_parent = poms[ws_idx].data.parent.clone();
            continue;
        }
        if pv.contains("${") || !seen_repo.insert((pg.clone(), pa.clone(), pv.clone())) {
            break;
        }
        let Some(parent_pom) =
            load_repo_pom_cached(repo_roots, &pg, &pa, &pv, repo_cache, loaded_parent_poms)
                .cloned()
        else {
            break;
        };
        next_parent = parent_pom.parent.clone();
        chain.push(parent_pom);
    }

    chain.reverse();
    chain
}

#[allow(clippy::too_many_arguments)]
fn expand_bom_managed(
    group: &str,
    artifact: &str,
    version: &str,
    inherited_props: &BTreeMap<String, String>,
    repo_roots: &[String],
    repo_cache: &mut HashMap<(String, String, String), Option<PomData>>,
    loaded_parent_poms: &mut HashSet<(String, String, String)>,
    visited_boms: &mut HashSet<(String, String, String)>,
    depth: u8,
    out_managed: &mut HashMap<(String, String), String>,
) {
    if depth > 2
        || version.contains("${")
        || !visited_boms.insert((group.to_string(), artifact.to_string(), version.to_string()))
    {
        return;
    }
    let Some(bom) = load_repo_pom_cached(
        repo_roots,
        group,
        artifact,
        version,
        repo_cache,
        loaded_parent_poms,
    )
    .cloned() else {
        return;
    };
    let mut bom_props = inherited_props.clone();
    for (k, v) in &bom.properties {
        if matches!(
            k.as_str(),
            "project.version" | "project.groupId" | "project.parent.version"
        ) {
            bom_props.insert(k.clone(), v.clone());
        } else {
            bom_props.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let mut nested_boms = Vec::new();
    for (mg, ma, mv, ms) in &bom.managed_deps {
        let g_res = interpolate_props(mg, &bom_props);
        let a_res = interpolate_props(ma, &bom_props);
        let Some(mv) = mv else {
            continue;
        };
        let v_res = interpolate_props(mv, &bom_props);
        if v_res.contains("${") || v_res.is_empty() {
            continue;
        }
        if ms.as_deref() == Some("import") {
            nested_boms.push((g_res, a_res, v_res));
        } else {
            out_managed.entry((g_res, a_res)).or_insert(v_res);
        }
    }
    for (bg, ba, bv) in nested_boms {
        expand_bom_managed(
            &bg,
            &ba,
            &bv,
            &bom_props,
            repo_roots,
            repo_cache,
            loaded_parent_poms,
            visited_boms,
            depth + 1,
            out_managed,
        );
    }
}

struct EffectivePom {
    props: BTreeMap<String, String>,
    effective_managed: HashMap<(String, String), String>,
    own_managed: HashMap<(String, String), String>,
}

fn resolve_effective_pom(
    chain: &[PomData],
    repo_roots: &[String],
    repo_cache: &mut HashMap<(String, String, String), Option<PomData>>,
    loaded_parent_poms: &mut HashSet<(String, String, String)>,
) -> EffectivePom {
    let mut effective_props = BTreeMap::new();
    for pom in chain {
        for (k, v) in &pom.properties {
            effective_props.insert(k.clone(), v.clone());
        }
    }

    let mut effective_managed: HashMap<(String, String), String> = HashMap::new();
    let mut parent_snapshot: HashMap<(String, String), String> = HashMap::new();
    let mut visited_boms = HashSet::new();

    for (idx, pom) in chain.iter().enumerate() {
        let is_current = idx + 1 == chain.len();
        if is_current {
            parent_snapshot = effective_managed.clone();
        }
        let mut step_props = effective_props.clone();
        for (k, v) in &pom.properties {
            if matches!(
                k.as_str(),
                "project.version" | "project.groupId" | "project.parent.version"
            ) {
                step_props.insert(k.clone(), v.clone());
            }
        }
        let mut bom_managed = HashMap::new();
        for (mg, ma, mv, ms) in &pom.managed_deps {
            if ms.as_deref() != Some("import") {
                continue;
            }
            let g_res = interpolate_props(mg, &step_props);
            let a_res = interpolate_props(ma, &step_props);
            let Some(mv) = mv else {
                continue;
            };
            let v_res = interpolate_props(mv, &step_props);
            if !v_res.contains("${") && !v_res.is_empty() {
                expand_bom_managed(
                    &g_res,
                    &a_res,
                    &v_res,
                    &step_props,
                    repo_roots,
                    repo_cache,
                    loaded_parent_poms,
                    &mut visited_boms,
                    0,
                    &mut bom_managed,
                );
            }
        }
        for (coord, ver) in bom_managed {
            effective_managed.insert(coord, ver);
        }
        for (mg, ma, mv, ms) in &pom.managed_deps {
            if ms.as_deref() == Some("import") {
                continue;
            }
            let g_res = interpolate_props(mg, &step_props);
            let a_res = interpolate_props(ma, &step_props);
            let Some(mv) = mv else {
                continue;
            };
            let v_res = interpolate_props(mv, &step_props);
            if !v_res.contains("${") && !v_res.is_empty() {
                effective_managed.insert((g_res, a_res), v_res);
            }
        }
    }

    let own_managed = effective_managed
        .iter()
        .filter(|(k, v)| parent_snapshot.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    EffectivePom {
        props: effective_props,
        effective_managed,
        own_managed,
    }
}

fn collect_project_deps(project: &host::Project, repo_roots: &[String]) -> ProjectDeps {
    let mut out = ProjectDeps::default();
    let mut poms = Vec::new();
    let mut fp = 0xcbf29ce484222325u64;
    if let Ok(mut files) = project.find_files("**/pom.xml", MAX_POM_FILES) {
        files.sort();
        for path in files {
            if !source::ignored_path(&path)
                && let Ok(text) = project.read_file(&path)
            {
                out.has_pom = true;
                fp = fnv1a_update(fp, path.as_bytes());
                fp = fnv1a_update(fp, &[0]);
                fp = fnv1a_update(fp, text.as_bytes());
                let data = parse_pom(&text);
                if let Some(coord) = &data.self_coord {
                    out.workspace_modules.insert(coord.clone());
                }
                let norm = path.trim_start_matches('/');
                let depth = norm.bytes().filter(|&b| b == b'/').count();
                poms.push(WorkspacePom {
                    depth,
                    path: norm.to_string(),
                    data,
                });
            }
        }
    }
    out.pom_fingerprint = fp;
    poms.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.path.cmp(&b.path)));

    let mut workspace_by_coord: HashMap<(String, String), usize> = HashMap::new();
    for (idx, pom) in poms.iter().enumerate() {
        if let Some(coord) = &pom.data.self_coord {
            workspace_by_coord.entry(coord.clone()).or_insert(idx);
        }
    }

    let mut repo_cache: HashMap<(String, String, String), Option<PomData>> = HashMap::new();
    let mut declared_map: BTreeMap<(String, String), Option<String>> = BTreeMap::new();

    for idx in 0..poms.len() {
        let chain = collect_pom_chain(
            idx,
            &poms,
            &workspace_by_coord,
            repo_roots,
            &mut repo_cache,
            &mut out.loaded_parent_poms,
        );
        let eff = resolve_effective_pom(
            &chain,
            repo_roots,
            &mut repo_cache,
            &mut out.loaded_parent_poms,
        );
        for (coord, ver) in eff.effective_managed {
            out.preferred_versions.entry(coord).or_insert(ver);
        }
        for (coord, ver) in eff.own_managed {
            out.preferred_versions.insert(coord, ver);
        }
        for (g, a, v) in &poms[idx].data.deps {
            let g_res = interpolate_props(g, &eff.props);
            let a_res = interpolate_props(a, &eff.props);
            let v_res = v
                .as_deref()
                .map(|raw| interpolate_props(raw, &eff.props))
                .filter(|s| !s.contains("${") && !s.is_empty())
                .or_else(|| {
                    out.preferred_versions
                        .get(&(g_res.clone(), a_res.clone()))
                        .cloned()
                });
            if let Some(ver) = &v_res {
                out.preferred_versions
                    .insert((g_res.clone(), a_res.clone()), ver.clone());
            }
            let coord = (g_res, a_res);
            match declared_map.get_mut(&coord) {
                Some(slot) => {
                    if v_res.is_some() {
                        *slot = v_res;
                    }
                }
                None => {
                    declared_map.insert(coord, v_res);
                }
            }
        }
    }
    for ((g, a), v) in declared_map {
        out.declared_deps.push((g, a, v));
    }

    collect_gradle_deps(project, &mut out);
    out
}

fn collect_gradle_deps(project: &host::Project, out: &mut ProjectDeps) {
    let mut catalog_versions = HashMap::new();
    if let Ok(files) = project.find_files("**/libs.versions.toml", 20) {
        for path in files {
            if let Ok(text) = project.read_file(&path) {
                let mut section = "";
                for line in text.lines() {
                    let line = line.trim();
                    if line.starts_with('[') && line.ends_with(']') {
                        section = &line[1..line.len() - 1];
                        continue;
                    }
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if section == "versions"
                        && let Some((k, v)) = line.split_once('=')
                    {
                        let ver = v.trim().trim_matches(['"', '\'']);
                        if !ver.is_empty() {
                            catalog_versions.insert(k.trim().to_string(), ver.to_string());
                        }
                    } else if section == "libraries"
                        && let Some((_, v)) = line.split_once('=')
                    {
                        parse_gradle_library_toml(v.trim(), &catalog_versions, out);
                    }
                }
            }
        }
    }
    for glob in ["**/build.gradle", "**/build.gradle.kts"] {
        if let Ok(files) = project.find_files(glob, 100) {
            for path in files {
                if !source::ignored_path(&path)
                    && let Ok(text) = project.read_file(&path)
                {
                    extract_gradle_coordinates(&text, out);
                }
            }
        }
    }
}

fn parse_gradle_library_toml(rhs: &str, versions: &HashMap<String, String>, out: &mut ProjectDeps) {
    if rhs.starts_with('"') || rhs.starts_with('\'') {
        let s = rhs.trim_matches(['"', '\'']);
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 3 {
            out.preferred_versions
                .entry((parts[0].to_string(), parts[1].to_string()))
                .or_insert_with(|| parts[2].to_string());
            out.declared_deps.push((
                parts[0].to_string(),
                parts[1].to_string(),
                Some(parts[2].to_string()),
            ));
        }
        return;
    }
    let find_str = |key: &str| -> Option<String> {
        let idx = rhs.find(key)?;
        let after = rhs[idx + key.len()..].trim_start();
        let after = after.strip_prefix('=')?.trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let end = after[1..].find(quote)?;
        Some(after[1..1 + end].to_string())
    };
    let Some(module) = find_str("module") else {
        return;
    };
    let Some((g, a)) = module.split_once(':') else {
        return;
    };
    let ver = find_str("version.ref")
        .and_then(|r| versions.get(&r).cloned())
        .or_else(|| find_str("version"));
    if let Some(v) = &ver {
        out.preferred_versions
            .entry((g.to_string(), a.to_string()))
            .or_insert_with(|| v.clone());
    }
    out.declared_deps.push((g.to_string(), a.to_string(), ver));
}

fn extract_gradle_coordinates(text: &str, out: &mut ProjectDeps) {
    for quote in ['"', '\''] {
        let mut rest = text;
        while let Some(s) = rest.find(quote) {
            let after = &rest[s + 1..];
            let Some(e) = after.find(quote) else {
                break;
            };
            let literal = &after[..e];
            rest = &after[e + 1..];
            let parts: Vec<&str> = literal.split(':').collect();
            if parts.len() == 3
                && parts[0].contains('.')
                && !parts[0].contains(['$', ' ', '/'])
                && !parts[1].contains(['$', ' ', '/'])
                && !parts[2].contains(['$', ' ', '/'])
                && !parts[2].is_empty()
            {
                out.preferred_versions
                    .entry((parts[0].to_string(), parts[1].to_string()))
                    .or_insert_with(|| parts[2].to_string());
                out.declared_deps.push((
                    parts[0].to_string(),
                    parts[1].to_string(),
                    Some(parts[2].to_string()),
                ));
            }
        }
    }
}

fn read_repo_pom(
    repo_roots: &[String],
    group: &str,
    artifact: &str,
    version: &str,
) -> Option<String> {
    let group_path = group.replace('.', "/");
    for root in repo_roots {
        let dir = format!("{root}/{group_path}/{artifact}/{version}");
        let exact = format!("{dir}/{artifact}-{version}.pom");
        if let Ok(text) = host::fs::read_file(&exact) {
            return Some(text);
        }
        if let Ok(entries) = host::fs::read_dir(&dir) {
            for e in entries {
                if e.name.ends_with(".pom")
                    && let Ok(text) = host::fs::read_file(&format!("{dir}/{}", e.name))
                {
                    return Some(text);
                }
            }
        }
    }
    None
}

struct LoadedJar {
    label: String,
    main_bytes: Vec<u8>,
    main_index: ZipIndex,
    sources_path: Option<String>,
    sources_bytes: Option<Vec<u8>>,
    sources_index: Option<ZipIndex>,
    sources_checked: bool,
}

struct JarCache {
    repo_roots: Vec<String>,
    matched_jars: HashMap<String, LoadedJar>,
    probed_paths: HashSet<String>,
    probed_count: usize,
    class_to_jar_path: HashMap<String, (String, Option<String>)>,
    probed_group_jars: HashMap<String, Vec<(String, String, Option<String>)>>,
    total_bytes: usize,
}

impl JarCache {
    fn new(repo_roots: Vec<String>) -> Self {
        Self {
            repo_roots,
            matched_jars: HashMap::new(),
            probed_paths: HashSet::new(),
            probed_count: 0,
            class_to_jar_path: HashMap::new(),
            probed_group_jars: HashMap::new(),
            total_bytes: 0,
        }
    }

    fn loaded_jar_count(&self) -> usize {
        self.matched_jars.len()
    }

    fn can_retain_more(&self) -> bool {
        self.matched_jars.len() < MAX_JARS && self.total_bytes < MAX_TOTAL_JAR_BYTES
    }

    fn can_probe_more(&self) -> bool {
        self.probed_count < MAX_PROBED_JARS && self.can_retain_more()
    }

    fn resolve_fqn(
        &mut self,
        fqn: &str,
        deps: &mut ProjectDeps,
        task: Option<&host::Task>,
    ) -> Result<Vec<Class>, Error> {
        check_cancelled(task)?;
        let (pkg, cls) = match fqn.rsplit_once('.') {
            Some((p, c)) => (p, c),
            None => return Ok(Vec::new()),
        };
        let mut class_entries = vec![format!(
            "{}/{}.class",
            pkg.replace('.', "/"),
            cls.replace('.', "$")
        )];
        let mut source_entries = vec![format!("{}/{}.java", pkg.replace('.', "/"), cls)];
        if let Some((outer_pkg, inner_cls)) = pkg.rsplit_once('.') {
            class_entries.push(format!(
                "{}/{}${}.class",
                outer_pkg.replace('.', "/"),
                inner_cls,
                cls
            ));
            source_entries.push(format!(
                "{}/{}.java",
                outer_pkg.replace('.', "/"),
                inner_cls
            ));
        }

        if let Some(found) = self.extract_from_indexed(&class_entries, &source_entries, fqn) {
            return Ok(found);
        }

        if !self.can_probe_more() {
            return Ok(Vec::new());
        }

        let candidates = self.candidate_jar_pairs(pkg, deps);
        for (main_jar, sources_jar) in candidates {
            check_cancelled(task)?;
            if !self.can_probe_more() {
                break;
            }
            if !self.probe_jar(&main_jar, sources_jar, &class_entries) {
                continue;
            }
            if let Some(found) = self.extract_from_indexed(&class_entries, &source_entries, fqn) {
                return Ok(found);
            }
        }
        Ok(Vec::new())
    }

    fn extract_from_indexed(
        &mut self,
        class_entries: &[String],
        source_entries: &[String],
        fqn: &str,
    ) -> Option<Vec<Class>> {
        let (matched_class_entry, main_path, sources_path) =
            class_entries.iter().find_map(|entry| {
                self.class_to_jar_path
                    .get(entry)
                    .map(|(m, s)| (entry.clone(), m.clone(), s.clone()))
            })?;

        if !self.matched_jars.contains_key(&main_path) {
            if !self.can_retain_more() {
                return None;
            }
            let stat = host::fs::stat(&main_path).ok()?;
            if stat.size > MAX_JAR_BYTES
                || self.total_bytes + stat.size as usize > MAX_TOTAL_JAR_BYTES
            {
                return None;
            }
            let bytes = host::fs::read_bytes(&main_path).ok()?;
            let index = ZipIndex::parse(&bytes)?;
            self.retain_jar(&main_path, sources_path, bytes, index);
        }

        self.extract_from_retained(&main_path, &matched_class_entry, source_entries, fqn)
    }

    fn extract_from_retained(
        &mut self,
        main_path: &str,
        matched_class_entry: &str,
        source_entries: &[String],
        fqn: &str,
    ) -> Option<Vec<Class>> {
        let (label, src_path_opt, sources_checked) = {
            let jar = self.matched_jars.get(main_path)?;
            (
                format!("{} ({fqn})", jar.label),
                jar.sources_path.clone(),
                jar.sources_checked,
            )
        };

        if !sources_checked {
            if let Some(jar) = self.matched_jars.get_mut(main_path) {
                jar.sources_checked = true;
            }
            if let Some(src_path) = src_path_opt
                && let Ok(stat) = host::fs::stat(&src_path)
                && stat.size <= MAX_JAR_BYTES
                && self.total_bytes + stat.size as usize <= MAX_TOTAL_JAR_BYTES
                && let Ok(bytes) = host::fs::read_bytes(&src_path)
            {
                let src_index = ZipIndex::parse(&bytes);
                self.total_bytes += bytes.len();
                if let Some(jar) = self.matched_jars.get_mut(main_path) {
                    jar.sources_bytes = Some(bytes);
                    jar.sources_index = src_index;
                }
            }
        }

        let jar = self.matched_jars.get(main_path)?;
        if let (Some(src_bytes), Some(src_index)) =
            (jar.sources_bytes.as_deref(), jar.sources_index.as_ref())
        {
            for src_entry in source_entries {
                if let Some(raw) =
                    src_index.read_entry(src_bytes, src_entry, DEFAULT_MAX_ENTRY_BYTES)
                    && let Ok(text) = String::from_utf8(raw)
                {
                    let mut parsed = source::parse(&label, &text);
                    for c in &mut parsed {
                        c.is_external = true;
                        if c.is_interface {
                            classfile::synthesize_interface_fields(c);
                        }
                    }
                    if parsed.iter().any(|c| c.qualified() == fqn) {
                        return Some(parsed);
                    }
                }
            }
        }

        let class_bytes = jar.main_index.read_entry(
            &jar.main_bytes,
            matched_class_entry,
            DEFAULT_MAX_ENTRY_BYTES,
        )?;
        let cls = classfile::parse_classfile(&class_bytes, &label)?;
        Some(vec![cls])
    }

    fn retain_jar(
        &mut self,
        main_path: &str,
        sources_path: Option<String>,
        bytes: Vec<u8>,
        index: ZipIndex,
    ) {
        if self.matched_jars.contains_key(main_path) {
            return;
        }
        self.total_bytes += bytes.len();
        let label = main_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(main_path)
            .to_string();
        self.matched_jars.insert(
            main_path.to_string(),
            LoadedJar {
                label,
                main_bytes: bytes,
                main_index: index,
                sources_path,
                sources_bytes: None,
                sources_index: None,
                sources_checked: false,
            },
        );
    }

    /// 探测候选 JAR：只把 `.class` 目录写入轻量索引；未命中所需类时立即丢弃字节，不占配额。
    fn probe_jar(
        &mut self,
        main_path: &str,
        sources_path: Option<String>,
        target_entries: &[String],
    ) -> bool {
        if !self.probed_paths.insert(main_path.to_string()) {
            return false;
        }
        let Ok(stat) = host::fs::stat(main_path) else {
            return false;
        };
        if stat.size > MAX_JAR_BYTES {
            return false;
        }
        let bytes = match host::fs::read_bytes(main_path) {
            Ok(b) => b,
            Err(Error::Denied(_)) | Err(Error::Invalid(_)) | Err(_) => return false,
        };
        self.probed_count += 1;
        let Some(index) = ZipIndex::parse(&bytes) else {
            return false;
        };
        let mut hit = false;
        for name in index.entry_names() {
            if name.ends_with(".class") {
                if !hit && target_entries.iter().any(|t| t == name) {
                    hit = true;
                }
                self.class_to_jar_path
                    .entry(name.to_string())
                    .or_insert_with(|| (main_path.to_string(), sources_path.clone()));
            }
        }
        if hit && self.total_bytes + bytes.len() <= MAX_TOTAL_JAR_BYTES {
            self.retain_jar(main_path, sources_path, bytes, index);
        }
        hit
    }

    fn candidate_jar_pairs(
        &mut self,
        pkg: &str,
        deps: &ProjectDeps,
    ) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut push_pair = |pair: (String, Option<String>)| {
            if seen.insert(pair.0.clone()) {
                out.push(pair);
            }
        };

        // 1. 优先匹配 groupId 是包前缀的显式项目依赖
        let mut matching_deps: Vec<&(String, String, Option<String>)> = deps
            .declared_deps
            .iter()
            .filter(|(g, _, _)| pkg == g || pkg.starts_with(&format!("{g}.")))
            .collect();
        matching_deps.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
        for (g, a, v) in matching_deps {
            for pair in self.find_artifact_jars(g, a, v.as_deref(), deps) {
                push_pair(pair);
            }
        }

        // 2. 按包前 3 段 / 2 段扫描本地 Maven 仓库 group 目录
        let segs: Vec<&str> = pkg.split('.').collect();
        let mut group_candidates = Vec::new();
        if segs.len() >= 3 {
            group_candidates.push(segs[..3].join("."));
        }
        if segs.len() >= 2 {
            group_candidates.push(segs[..2].join("."));
        }
        if segs.len() >= 4 {
            group_candidates.push(segs[..4].join("."));
        }
        for group in &group_candidates {
            for pair in self.probe_group_dir(group, &segs, deps) {
                push_pair(pair);
            }
        }

        // 3. 补充与目标包共享前缀或 artifactId 命中的声明依赖，避免加载无关依赖耗尽配额
        let two_seg_prefix = if segs.len() >= 2 {
            Some(segs[..2].join("."))
        } else {
            None
        };
        for (g, a, v) in &deps.declared_deps {
            if !(pkg == g || pkg.starts_with(&format!("{g}."))) {
                let shares_prefix = two_seg_prefix
                    .as_deref()
                    .is_some_and(|p| g == p || g.starts_with(&format!("{p}.")));
                let artifact_matches = segs.iter().any(|seg| seg.len() >= 3 && a.contains(seg));
                if shares_prefix || artifact_matches {
                    for pair in self.find_artifact_jars(g, a, v.as_deref(), deps) {
                        push_pair(pair);
                    }
                }
            }
        }

        out
    }

    fn find_artifact_jars(
        &self,
        group: &str,
        artifact: &str,
        version: Option<&str>,
        deps: &ProjectDeps,
    ) -> Vec<(String, Option<String>)> {
        if deps
            .workspace_modules
            .contains(&(group.to_string(), artifact.to_string()))
        {
            return Vec::new();
        }
        let mut out = Vec::new();
        let group_path = group.replace('.', "/");
        let resolved_ver = version.map(str::to_string).or_else(|| {
            deps.preferred_versions
                .get(&(group.to_string(), artifact.to_string()))
                .cloned()
        });

        for root in &self.repo_roots {
            // Maven 布局：<root>/<group_path>/<artifact>/<version>/
            let m2_artifact_dir = format!("{root}/{group_path}/{artifact}");
            let mut m2_hit = false;
            if let Some(ver) = &resolved_ver {
                let ver_dir = format!("{m2_artifact_dir}/{ver}");
                if let Some(pair) = pick_jars_in_version_dir(&ver_dir, artifact) {
                    out.push(pair);
                    m2_hit = true;
                }
            }
            if !m2_hit && let Ok(entries) = host::fs::read_dir(&m2_artifact_dir) {
                let mut versions: Vec<String> = entries
                    .into_iter()
                    .filter(|e| e.kind == openidea_plugin::FileKind::Directory)
                    .map(|e| e.name)
                    .collect();
                versions.sort_by(|a, b| b.cmp(a));
                for ver in versions {
                    let ver_dir = format!("{m2_artifact_dir}/{ver}");
                    if let Some(pair) = pick_jars_in_version_dir(&ver_dir, artifact) {
                        out.push(pair);
                        break;
                    }
                }
            }

            // Gradle 布局：<root>/<group>/<artifact>/<version>/<hash>/
            let gradle_artifact_dir = format!("{root}/{group}/{artifact}");
            let mut gradle_hit = false;
            if let Some(ver) = &resolved_ver {
                let ver_dir = format!("{gradle_artifact_dir}/{ver}");
                let pairs = pick_gradle_jars_in_version_dir(&ver_dir, artifact);
                if !pairs.is_empty() {
                    out.extend(pairs);
                    gradle_hit = true;
                }
            }
            if !gradle_hit && let Ok(entries) = host::fs::read_dir(&gradle_artifact_dir) {
                let mut versions: Vec<String> = entries
                    .into_iter()
                    .filter(|e| e.kind == openidea_plugin::FileKind::Directory)
                    .map(|e| e.name)
                    .collect();
                versions.sort_by(|a, b| b.cmp(a));
                for ver in versions {
                    let ver_dir = format!("{gradle_artifact_dir}/{ver}");
                    let pairs = pick_gradle_jars_in_version_dir(&ver_dir, artifact);
                    if !pairs.is_empty() {
                        out.extend(pairs);
                        break;
                    }
                }
            }
        }
        out
    }

    fn probe_group_dir(
        &mut self,
        group: &str,
        pkg_segs: &[&str],
        deps: &ProjectDeps,
    ) -> Vec<(String, Option<String>)> {
        let mut collected: Vec<(String, String, Option<String>)> = Vec::new();
        let group_path = group.replace('.', "/");
        for root in self.repo_roots.clone() {
            // Maven 布局
            let m2_group_dir = format!("{root}/{group_path}");
            if !self.probed_group_jars.contains_key(&m2_group_dir) {
                let mut found = Vec::new();
                if let Ok(entries) = host::fs::read_dir(&m2_group_dir) {
                    let mut artifacts: Vec<String> = entries
                        .into_iter()
                        .filter(|e| e.kind == openidea_plugin::FileKind::Directory)
                        .map(|e| e.name)
                        .collect();
                    artifacts.sort();
                    for artifact in artifacts.into_iter().take(48) {
                        if deps
                            .workspace_modules
                            .contains(&(group.to_string(), artifact.clone()))
                        {
                            continue;
                        }
                        let ver = deps
                            .preferred_versions
                            .get(&(group.to_string(), artifact.clone()))
                            .map(String::as_str);
                        let m2_artifact_dir = format!("{m2_group_dir}/{artifact}");
                        let mut m2_hit = false;
                        if let Some(v) = ver {
                            let ver_dir = format!("{m2_artifact_dir}/{v}");
                            if let Some((main, src)) = pick_jars_in_version_dir(&ver_dir, &artifact)
                            {
                                found.push((artifact.clone(), main, src));
                                m2_hit = true;
                            }
                        }
                        if !m2_hit && let Ok(ver_entries) = host::fs::read_dir(&m2_artifact_dir) {
                            let mut versions: Vec<String> = ver_entries
                                .into_iter()
                                .filter(|e| e.kind == openidea_plugin::FileKind::Directory)
                                .map(|e| e.name)
                                .collect();
                            versions.sort_by(|a, b| b.cmp(a));
                            for v in versions {
                                let ver_dir = format!("{m2_artifact_dir}/{v}");
                                if let Some((main, src)) =
                                    pick_jars_in_version_dir(&ver_dir, &artifact)
                                {
                                    found.push((artifact.clone(), main, src));
                                    break;
                                }
                            }
                        }
                    }
                }
                self.probed_group_jars.insert(m2_group_dir.clone(), found);
            }
            if let Some(cached) = self.probed_group_jars.get(&m2_group_dir) {
                collected.extend(cached.iter().cloned());
            }

            // Gradle 布局
            let gradle_group_dir = format!("{root}/{group}");
            if !self.probed_group_jars.contains_key(&gradle_group_dir) {
                let mut found = Vec::new();
                if let Ok(entries) = host::fs::read_dir(&gradle_group_dir) {
                    let mut artifacts: Vec<String> = entries
                        .into_iter()
                        .filter(|e| e.kind == openidea_plugin::FileKind::Directory)
                        .map(|e| e.name)
                        .collect();
                    artifacts.sort();
                    for artifact in artifacts.into_iter().take(48) {
                        if deps
                            .workspace_modules
                            .contains(&(group.to_string(), artifact.clone()))
                        {
                            continue;
                        }
                        let gradle_artifact_dir = format!("{gradle_group_dir}/{artifact}");
                        if let Ok(ver_entries) = host::fs::read_dir(&gradle_artifact_dir) {
                            let mut versions: Vec<String> = ver_entries
                                .into_iter()
                                .filter(|e| e.kind == openidea_plugin::FileKind::Directory)
                                .map(|e| e.name)
                                .collect();
                            versions.sort_by(|a, b| b.cmp(a));
                            for v in versions {
                                let pairs = pick_gradle_jars_in_version_dir(
                                    &format!("{gradle_artifact_dir}/{v}"),
                                    &artifact,
                                );
                                if !pairs.is_empty() {
                                    for (main, src) in pairs {
                                        found.push((artifact.clone(), main, src));
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
                self.probed_group_jars
                    .insert(gradle_group_dir.clone(), found);
            }
            if let Some(cached) = self.probed_group_jars.get(&gradle_group_dir) {
                collected.extend(cached.iter().cloned());
            }
        }
        collected.sort_by(|a, b| {
            let score = |name: &str| -> usize {
                pkg_segs
                    .iter()
                    .filter(|seg| seg.len() >= 3 && name.contains(*seg))
                    .count()
            };
            score(&b.0).cmp(&score(&a.0)).then_with(|| a.0.cmp(&b.0))
        });
        collected
            .into_iter()
            .map(|(_, main, src)| (main, src))
            .collect()
    }
}

fn is_main_jar(name: &str) -> bool {
    name.ends_with(".jar")
        && !name.ends_with("-sources.jar")
        && !name.ends_with("-javadoc.jar")
        && !name.ends_with("-tests.jar")
        && !name.ends_with("-test-sources.jar")
}

fn pick_jars_in_version_dir(ver_dir: &str, artifact: &str) -> Option<(String, Option<String>)> {
    let entries = host::fs::read_dir(ver_dir).ok()?;
    let mut main_jars = Vec::new();
    let mut source_jars = Vec::new();
    for e in entries {
        if e.kind != openidea_plugin::FileKind::File {
            continue;
        }
        if e.name.ends_with("-sources.jar") {
            source_jars.push(e.name);
        } else if is_main_jar(&e.name) {
            main_jars.push(e.name);
        }
    }
    main_jars.sort_by(|a, b| {
        let a_snapshot = a.ends_with("-SNAPSHOT.jar");
        let b_snapshot = b.ends_with("-SNAPSHOT.jar");
        let a_pref = a.starts_with(artifact);
        let b_pref = b.starts_with(artifact);
        b_pref
            .cmp(&a_pref)
            .then_with(|| b_snapshot.cmp(&a_snapshot))
            .then_with(|| b.cmp(a))
    });
    let main = main_jars.into_iter().next()?;
    source_jars.sort_by(|a, b| {
        let a_snapshot = a.ends_with("-SNAPSHOT-sources.jar");
        let b_snapshot = b.ends_with("-SNAPSHOT-sources.jar");
        b_snapshot.cmp(&a_snapshot).then_with(|| b.cmp(a))
    });
    let src = source_jars
        .into_iter()
        .next()
        .map(|s| format!("{ver_dir}/{s}"));
    Some((format!("{ver_dir}/{main}"), src))
}

fn pick_gradle_jars_in_version_dir(ver_dir: &str, artifact: &str) -> Vec<(String, Option<String>)> {
    let Ok(hashes) = host::fs::read_dir(ver_dir) else {
        return Vec::new();
    };
    let mut main_jars = Vec::new();
    let mut source_jars = Vec::new();
    for h in hashes {
        if h.kind != openidea_plugin::FileKind::Directory {
            continue;
        }
        let hash_dir = format!("{ver_dir}/{}", h.name);
        let Ok(files) = host::fs::read_dir(&hash_dir) else {
            continue;
        };
        for f in files {
            if f.kind != openidea_plugin::FileKind::File {
                continue;
            }
            if f.name.ends_with("-sources.jar") {
                source_jars.push(format!("{hash_dir}/{}", f.name));
            } else if is_main_jar(&f.name) && f.name.starts_with(artifact) {
                main_jars.push(format!("{hash_dir}/{}", f.name));
            }
        }
    }
    let src = source_jars.into_iter().next();
    main_jars.into_iter().map(|m| (m, src.clone())).collect()
}

fn parse_classpath_entries(raw: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let sep = if raw.contains(';') || raw.contains(":\\") || raw.contains(":/") {
        ';'
    } else {
        ':'
    };
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        for part in line.split(sep) {
            let item = part.trim();
            if item.is_empty() {
                continue;
            }
            if (item.ends_with(".jar") || item.contains('/') || item.contains('\\'))
                && seen.insert(item.to_string())
            {
                entries.push(item.to_string());
            }
        }
    }
    entries
}

fn maven_classpath(
    project: &host::Project,
    fingerprint: u64,
    task: Option<&host::Task>,
) -> Result<Option<String>, Error> {
    check_cancelled(task)?;
    if FAILED_MVN_FINGERPRINTS.with(|s| s.borrow().contains(&fingerprint)) {
        return Ok(None);
    }
    let plugin_dir = host::fs::plugin_dir().ok();
    let cache_path = plugin_dir
        .as_ref()
        .map(|d| format!("{d}/maven-cp-{fingerprint:016x}.txt"));
    if let Some(cp_path) = &cache_path
        && let Ok(cached) = host::fs::read_file(cp_path)
        && !cached.trim().is_empty()
    {
        return Ok(Some(cached));
    }

    if let Some(t) = task {
        t.report(0, None, Some("正在通过 Maven 获取依赖类路径..."));
    }

    let tmp_path = plugin_dir
        .as_ref()
        .map(|d| format!("{d}/maven-cp-{fingerprint:016x}.raw"));
    if let Some(tmp) = &tmp_path {
        let _ = host::fs::write_file(tmp, "");
    }
    let mut args = vec![
        "-o".to_string(),
        "-q".to_string(),
        "-fn".to_string(),
        "-DincludeScope=compile".to_string(),
    ];
    if let Some(tmp) = &tmp_path {
        args.push(format!("-Dmdep.outputFile={tmp}"));
        args.push("-Dmdep.appendOutput=true".to_string());
    }
    args.push("dependency:build-classpath".to_string());

    let output = match project.run_process("mvn", &args, None) {
        Ok(out) => out,
        Err(_) => {
            FAILED_MVN_FINGERPRINTS.with(|s| {
                s.borrow_mut().insert(fingerprint);
            });
            return Ok(None);
        }
    };
    let raw = tmp_path
        .as_deref()
        .and_then(|p| host::fs::read_file(p).ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(output.stdout);

    let entries = parse_classpath_entries(&raw);
    if entries.is_empty() {
        FAILED_MVN_FINGERPRINTS.with(|s| {
            s.borrow_mut().insert(fingerprint);
        });
        return Ok(None);
    }
    let sep = if entries
        .iter()
        .any(|e| e.contains('\\') || e.as_bytes().get(1) == Some(&b':'))
    {
        ";"
    } else {
        ":"
    };
    let joined = entries.join(sep);
    if let Some(cp_path) = &cache_path {
        let _ = host::fs::write_file(cp_path, &joined);
    }
    Ok(Some(joined))
}

fn fqn_to_jvm_name(fqn: &str) -> String {
    let parts: Vec<&str> = fqn.split('.').collect();
    if parts.len() >= 2
        && parts[parts.len() - 2]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
    {
        let mut out = parts[..parts.len() - 1].join(".");
        out.push('$');
        out.push_str(parts[parts.len() - 1]);
        return out;
    }
    fqn.to_string()
}

fn javap_batch(project: &host::Project, classpath: &str, batch: &[String]) -> Vec<Class> {
    let jvm_names: Vec<String> = batch.iter().map(|f| fqn_to_jvm_name(f)).collect();
    let mut args = Vec::new();
    if classpath.len() > 4096
        && let Ok(dir) = host::fs::plugin_dir()
    {
        let args_path = format!("{dir}/javap-cp.args");
        let escaped = classpath.replace('\\', "\\\\").replace('"', "\\\"");
        if host::fs::write_file(&args_path, &format!("-cp \"{escaped}\"\n")).is_ok() {
            args.push(format!("@{args_path}"));
        }
    }
    if args.is_empty() {
        args.push("-cp".to_string());
        args.push(classpath.to_string());
    }
    args.push("-p".to_string());
    args.push("-v".to_string());
    args.extend(jvm_names);

    let Ok(output) = project.run_process("javap", &args, None) else {
        return Vec::new();
    };
    if output.stdout.trim().is_empty() {
        return Vec::new();
    }
    classfile::parse_javap_verbose(&output.stdout)
}

#[allow(clippy::too_many_arguments)]
fn resolve_via_mvn_javap(
    project: &host::Project,
    deps: &ProjectDeps,
    scope: &Scope,
    all_classes: &mut Vec<Class>,
    known_fqns: &mut HashSet<String>,
    resolved_classes: &mut Vec<Class>,
    mut pending: Vec<String>,
    attempted_fqns: &mut HashSet<String>,
    task: Option<&host::Task>,
) -> Result<usize, Error> {
    let Some(classpath) = maven_classpath(project, deps.pom_fingerprint, task)? else {
        return Ok(0);
    };
    let mut javap_resolved = 0usize;
    for _round in 0..4 {
        check_cancelled(task)?;
        if pending.is_empty() || resolved_classes.len() >= MAX_CLASSES {
            break;
        }
        let chunks: Vec<&[String]> = pending.chunks(32).collect();
        let batch_total = chunks.len() as u32;
        let mut newly_added = false;
        for (idx, chunk) in chunks.into_iter().enumerate() {
            check_cancelled(task)?;
            if resolved_classes.len() >= MAX_CLASSES {
                break;
            }
            if let Some(t) = task {
                t.report(
                    (idx + 1) as u32,
                    Some(batch_total),
                    Some("正在通过 javap 解析外部类型..."),
                );
            }
            for cls in javap_batch(project, &classpath, chunk) {
                if known_fqns.insert(cls.qualified()) {
                    all_classes.push(cls.clone());
                    resolved_classes.push(cls);
                    javap_resolved += 1;
                    newly_added = true;
                    if resolved_classes.len() >= MAX_CLASSES {
                        break;
                    }
                }
            }
        }
        if !newly_added {
            break;
        }
        pending = collect_missing_fqns(all_classes, scope, attempted_fqns).all;
    }
    Ok(javap_resolved)
}
