use apifox_helper_openidea::{
    classfile::{
        DEFAULT_MAX_ENTRY_BYTES, ZipArchive, inflate_raw, parse_classfile, parse_javap_verbose,
    },
    external::resolve_external_classes_with_roots,
    openapi::{self, Scope},
    plugin::{ApifoxHelper, scan},
    source,
};
use openidea_plugin::{ProcessOutput, host, mock::MockHost};

fn write_external_bytes(h: &MockHost, path: &str, bytes: &[u8]) {
    h.add_external_file(path, "");
    host::fs::write_bytes(path, bytes).unwrap();
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// 构造确定性的未压缩（STORED）或 raw DEFLATE 块包装的 ZIP 字节流。
fn build_zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd = Vec::new();
    for &(name, data, use_stored_deflate) in entries {
        let local_offset = out.len() as u32;
        let (method, payload) = if use_stored_deflate {
            // RFC 1951 single stored deflate block: BFINAL=1, BTYPE=00, LEN, NLEN, data
            let mut buf = Vec::with_capacity(5 + data.len());
            buf.push(0x01);
            let len = data.len() as u16;
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&(!len).to_le_bytes());
            buf.extend_from_slice(data);
            (8u16, buf)
        } else {
            (0u16, data.to_vec())
        };
        let crc = crc32(data);
        let name_bytes = name.as_bytes();
        // Local File Header
        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&payload);

        // Central Directory Header
        cd.extend_from_slice(b"PK\x01\x02");
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&method.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&crc.to_le_bytes());
        cd.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&local_offset.to_le_bytes());
        cd.extend_from_slice(name_bytes);
    }
    let cd_offset = out.len() as u32;
    let cd_size = cd.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

struct CpBuilder {
    entries: Vec<Vec<u8>>,
}

impl CpBuilder {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
    fn utf8(&mut self, s: &str) -> u16 {
        let mut buf = Vec::new();
        buf.push(1);
        buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
        buf.extend_from_slice(s.as_bytes());
        self.entries.push(buf);
        self.entries.len() as u16
    }
    fn class(&mut self, internal: &str) -> u16 {
        let name_idx = self.utf8(internal);
        let mut buf = vec![7];
        buf.extend_from_slice(&name_idx.to_be_bytes());
        self.entries.push(buf);
        self.entries.len() as u16
    }
    fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&((self.entries.len() + 1) as u16).to_be_bytes());
        for e in &self.entries {
            out.extend_from_slice(e);
        }
    }
}

type MemberSpec<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<(&'a str, &'a str, &'a str)>,
);

/// 构造标准 JVM `.class` 字节码（支持泛型签名、字段、接口 getter 与 RuntimeVisibleAnnotations）。
fn build_simple_classfile(
    internal_name: &str,
    super_internal: &str,
    interfaces: &[&str],
    access_flags: u16,
    class_sig: Option<&str>,
    fields: &[MemberSpec<'_>],
    methods: &[MemberSpec<'_>],
) -> Vec<u8> {
    let mut cp = CpBuilder::new();
    let this_idx = cp.class(internal_name);
    let super_idx = cp.class(super_internal);
    let iface_idxs: Vec<u16> = interfaces.iter().map(|i| cp.class(i)).collect();
    let sig_attr_name = cp.utf8("Signature");
    let ann_attr_name = cp.utf8("RuntimeVisibleAnnotations");

    let mut field_bytes = Vec::new();
    field_bytes.extend_from_slice(&(fields.len() as u16).to_be_bytes());
    for &(fname, fdesc, fsig, fann) in fields {
        let name_idx = cp.utf8(fname);
        let desc_idx = cp.utf8(fdesc);
        let sig_val_idx = fsig.map(|s| cp.utf8(s));
        let ann_data = fann.map(|(adesc, key, val)| (cp.utf8(adesc), cp.utf8(key), cp.utf8(val)));
        let attr_count = sig_val_idx.is_some() as u16 + ann_data.is_some() as u16;
        field_bytes.extend_from_slice(&0x0002u16.to_be_bytes()); // private
        field_bytes.extend_from_slice(&name_idx.to_be_bytes());
        field_bytes.extend_from_slice(&desc_idx.to_be_bytes());
        field_bytes.extend_from_slice(&attr_count.to_be_bytes());
        if let Some(sidx) = sig_val_idx {
            field_bytes.extend_from_slice(&sig_attr_name.to_be_bytes());
            field_bytes.extend_from_slice(&2u32.to_be_bytes());
            field_bytes.extend_from_slice(&sidx.to_be_bytes());
        }
        if let Some((tidx, kidx, vidx)) = ann_data {
            field_bytes.extend_from_slice(&ann_attr_name.to_be_bytes());
            field_bytes.extend_from_slice(&11u32.to_be_bytes());
            field_bytes.extend_from_slice(&1u16.to_be_bytes()); // num_annotations
            field_bytes.extend_from_slice(&tidx.to_be_bytes());
            field_bytes.extend_from_slice(&1u16.to_be_bytes()); // num_element_value_pairs
            field_bytes.extend_from_slice(&kidx.to_be_bytes());
            field_bytes.push(b's');
            field_bytes.extend_from_slice(&vidx.to_be_bytes());
        }
    }

    let mut method_bytes = Vec::new();
    method_bytes.extend_from_slice(&(methods.len() as u16).to_be_bytes());
    for &(mname, mdesc, msig, mann) in methods {
        let name_idx = cp.utf8(mname);
        let desc_idx = cp.utf8(mdesc);
        let sig_val_idx = msig.map(|s| cp.utf8(s));
        let ann_data = mann.map(|(adesc, key, val)| (cp.utf8(adesc), cp.utf8(key), cp.utf8(val)));
        let attr_count = sig_val_idx.is_some() as u16 + ann_data.is_some() as u16;
        method_bytes.extend_from_slice(&0x0001u16.to_be_bytes()); // public
        method_bytes.extend_from_slice(&name_idx.to_be_bytes());
        method_bytes.extend_from_slice(&desc_idx.to_be_bytes());
        method_bytes.extend_from_slice(&attr_count.to_be_bytes());
        if let Some(sidx) = sig_val_idx {
            method_bytes.extend_from_slice(&sig_attr_name.to_be_bytes());
            method_bytes.extend_from_slice(&2u32.to_be_bytes());
            method_bytes.extend_from_slice(&sidx.to_be_bytes());
        }
        if let Some((tidx, kidx, vidx)) = ann_data {
            method_bytes.extend_from_slice(&ann_attr_name.to_be_bytes());
            method_bytes.extend_from_slice(&11u32.to_be_bytes());
            method_bytes.extend_from_slice(&1u16.to_be_bytes());
            method_bytes.extend_from_slice(&tidx.to_be_bytes());
            method_bytes.extend_from_slice(&1u16.to_be_bytes());
            method_bytes.extend_from_slice(&kidx.to_be_bytes());
            method_bytes.push(b's');
            method_bytes.extend_from_slice(&vidx.to_be_bytes());
        }
    }

    let class_sig_idx = class_sig.map(|s| cp.utf8(s));

    let mut out = Vec::new();
    out.extend_from_slice(&0xCAFEBABEu32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&61u16.to_be_bytes());
    cp.write_to(&mut out);
    out.extend_from_slice(&access_flags.to_be_bytes());
    out.extend_from_slice(&this_idx.to_be_bytes());
    out.extend_from_slice(&super_idx.to_be_bytes());
    out.extend_from_slice(&(iface_idxs.len() as u16).to_be_bytes());
    for i in iface_idxs {
        out.extend_from_slice(&i.to_be_bytes());
    }
    out.extend_from_slice(&field_bytes);
    out.extend_from_slice(&method_bytes);
    out.extend_from_slice(&(class_sig_idx.is_some() as u16).to_be_bytes());
    if let Some(sidx) = class_sig_idx {
        out.extend_from_slice(&sig_attr_name.to_be_bytes());
        out.extend_from_slice(&2u32.to_be_bytes());
        out.extend_from_slice(&sidx.to_be_bytes());
    }
    out
}

#[test]
fn zip_archive_and_deflate_handle_stored_fixed_huffman_and_limits() {
    // 1. Fixed Huffman literal stream: "hellohello" -> stored / fixed deflate check
    // Valid fixed Huffman block encoding "AAAABBBB":
    // 'A'=65 (0b01110001 in 8-bit fixed code: 0x30+65=113 -> 01110001 reversed = 10001110),
    // simpler: test stored deflate via ZipArchive + fixed Huffman empty/single-byte stream + size limit rejection.
    let zip_bytes = build_zip(&[
        ("a.txt", b"plain-stored", false),
        ("b.txt", b"deflate-stored-block", true),
    ]);
    let arc = ZipArchive::parse(&zip_bytes).expect("zip");
    assert_eq!(arc.read_entry("a.txt", 1024).unwrap(), b"plain-stored");
    assert_eq!(
        arc.read_entry("b.txt", 1024).unwrap(),
        b"deflate-stored-block"
    );
    // Size limit enforcement
    assert!(arc.read_entry("b.txt", 4).is_none());

    // Fixed Huffman block: BFINAL=1(1), BTYPE=01(01), 'a'(97 -> 0x30+97=145=10010001 rev=10001001), EOB(256 -> 0000000)
    // Bits LSB: 1, 1, 0, 1,0,0,0,1,0,0,1, 0,0,0,0,0,0,0
    // Byte 0: 1_0_0_1_0_0_1_1? Let's pack carefully:
    // bit0=1 (bfinal), bit1=1, bit2=0 (btype=01 => 0b01 LSB first: 1, 0)
    // Next 8 bits MSB-first for 145 (0b10010001): bits 1,0,0,1,0,0,0,1
    // Next 7 bits MSB-first for 0 (EOB=256): 0,0,0,0,0,0,0
    // Total 18 bits:
    // b0: [1,1,0, 1,0,0,1,0] -> LSB..MSB = 0b01001011 = 0x4B
    // b1: [0,0,1, 0,0,0,0,0] -> LSB..MSB = 0b00000100 = 0x04
    // b2: [0,0,...] = 0x00
    let fixed_a = inflate_raw(&[0x4B, 0x04, 0x00], 64).expect("fixed huffman");
    assert_eq!(fixed_a, b"a");
}

#[test]
fn classfile_parses_generic_class_and_interface_getters() {
    let pager_bytes = build_simple_classfile(
        "org/hswebframework/web/api/crud/entity/PagerResult",
        "java/lang/Object",
        &["java/io/Serializable"],
        0x0021,
        Some("<E:Ljava/lang/Object;>Ljava/lang/Object;Ljava/io/Serializable;"),
        &[
            (
                "total",
                "I",
                None,
                Some((
                    "Lio/swagger/v3/oas/annotations/media/Schema;",
                    "description",
                    "总数量",
                )),
            ),
            (
                "data",
                "Ljava/util/List;",
                Some("Ljava/util/List<TE;>;"),
                None,
            ),
        ],
        &[],
    );
    let cls = parse_classfile(&pager_bytes, "hsweb.jar (PagerResult)").expect("classfile");
    assert_eq!(
        cls.qualified(),
        "org.hswebframework.web.api.crud.entity.PagerResult"
    );
    assert_eq!(cls.generics, vec!["E"]);
    assert!(cls.bases.is_empty());
    assert_eq!(cls.fields.len(), 2);
    assert_eq!(cls.fields[0].name, "total");
    assert_eq!(cls.fields[0].ty, "int");
    assert_eq!(cls.fields[0].annotations[0].name, "Schema");
    assert_eq!(cls.fields[1].name, "data");
    assert_eq!(cls.fields[1].ty, "java.util.List<E>");

    let iface_bytes = build_simple_classfile(
        "org/jetlinks/core/metadata/ConfigMetadata",
        "java/lang/Object",
        &[
            "org/jetlinks/core/metadata/ConfigScopeSupport",
            "java/io/Serializable",
        ],
        0x0601, // public interface abstract
        None,
        &[],
        &[
            ("getName", "()Ljava/lang/String;", None, None),
            (
                "getProperties",
                "()Ljava/util/List;",
                Some("()Ljava/util/List<Lorg/jetlinks/core/metadata/ConfigPropertyMetadata;>;"),
                None,
            ),
        ],
    );
    let iface = parse_classfile(&iface_bytes, "jetlinks-core.jar (ConfigMetadata)").expect("iface");
    assert!(iface.is_interface);
    assert_eq!(
        iface.bases,
        vec!["org.jetlinks.core.metadata.ConfigScopeSupport"]
    );
    assert_eq!(iface.fields.len(), 2);
    assert_eq!(iface.fields[0].name, "name");
    assert_eq!(iface.fields[1].name, "properties");
    assert_eq!(
        iface.fields[1].ty,
        "java.util.List<org.jetlinks.core.metadata.ConfigPropertyMetadata>"
    );
}

#[test]
fn external_resolver_uses_sources_jar_first_and_classfile_fallback_with_mock_host() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "fs.read:/mock-m2",
        "fs.write:/mock-m2",
        "fs.read:/mock-gradle",
        "fs.write:/mock-gradle",
    ]);

    h.add_file(
        "pom.xml",
        r#"<project>
            <parent>
                <groupId>org.example</groupId>
                <artifactId>parent-pom</artifactId>
                <version>1.0.0</version>
            </parent>
            <dependencies>
                <dependency>
                    <groupId>org.hswebframework.web</groupId>
                    <artifactId>hsweb-commons-crud</artifactId>
                </dependency>
                <dependency>
                    <groupId>org.jetlinks</groupId>
                    <artifactId>jetlinks-core</artifactId>
                    <version>1.3.2-SNAPSHOT</version>
                </dependency>
            </dependencies>
        </project>"#,
    );

    h.add_file(
        "src/main/java/demo/DeviceController.java",
        r#"
package demo;
import org.hswebframework.web.api.crud.entity.PagerResult;
import org.jetlinks.core.metadata.ConfigMetadata;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/devices")
public class DeviceController {
    @GetMapping("/page")
    public PagerResult<DeviceEntity> page() { return null; }

    @GetMapping("/config")
    public ConfigMetadata config() { return null; }
}
"#,
    );
    h.add_file(
        "src/main/java/demo/DeviceEntity.java",
        r#"
package demo;
import org.hswebframework.web.api.crud.entity.GenericEntity;
public class DeviceEntity extends GenericEntity<String> {
    private String name;
}
"#,
    );

    // Parent POM in Maven repo provides managed version for hsweb-commons-crud
    h.add_external_file(
        "/mock-m2/org/example/parent-pom/1.0.0/parent-pom-1.0.0.pom",
        r#"<project>
            <dependencyManagement>
                <dependencies>
                    <dependency>
                        <groupId>org.hswebframework.web</groupId>
                        <artifactId>hsweb-commons-crud</artifactId>
                        <version>5.0.2-SNAPSHOT</version>
                    </dependency>
                </dependencies>
            </dependencyManagement>
        </project>"#,
    );

    // hsweb-commons-crud: has -sources.jar (preferred) + main .jar with a ghost @RestController that must be ignored
    let pager_class = build_simple_classfile(
        "org/hswebframework/web/api/crud/entity/PagerResult",
        "java/lang/Object",
        &[],
        0x0021,
        Some("<E:Ljava/lang/Object;>Ljava/lang/Object;"),
        &[("total", "I", None, None)],
        &[],
    );
    let generic_class = build_simple_classfile(
        "org/hswebframework/web/api/crud/entity/GenericEntity",
        "java/lang/Object",
        &[],
        0x0421,
        Some("<ID:Ljava/lang/Object;>Ljava/lang/Object;"),
        &[("id", "Ljava/lang/Object;", Some("TID;"), None)],
        &[],
    );
    let hsweb_main_jar = build_zip(&[
        (
            "org/hswebframework/web/api/crud/entity/PagerResult.class",
            &pager_class,
            false,
        ),
        (
            "org/hswebframework/web/api/crud/entity/GenericEntity.class",
            &generic_class,
            true,
        ),
    ]);
    let hsweb_sources_jar = build_zip(&[
        (
            "org/hswebframework/web/api/crud/entity/PagerResult.java",
            r#"package org.hswebframework.web.api.crud.entity;
import io.swagger.v3.oas.annotations.media.Schema;
import java.util.List;
@RestController
public class PagerResult<E> {
    @Schema(description = "总数来自源码包")
    private int total;
    private List<E> data;
}
"#
            .as_bytes(),
            true,
        ),
        (
            "org/hswebframework/web/api/crud/entity/GenericEntity.java",
            r#"package org.hswebframework.web.api.crud.entity;
public class GenericEntity<ID> {
    /** 主键ID */
    private ID id;
}
"#
            .as_bytes(),
            false,
        ),
    ]);
    write_external_bytes(
        &h,
        "/mock-m2/org/hswebframework/web/hsweb-commons-crud/5.0.2-SNAPSHOT/hsweb-commons-crud-5.0.2-20260721.114841-1.jar",
        &hsweb_main_jar,
    );
    write_external_bytes(
        &h,
        "/mock-m2/org/hswebframework/web/hsweb-commons-crud/5.0.2-SNAPSHOT/hsweb-commons-crud-5.0.2-20260721.114841-1-sources.jar",
        &hsweb_sources_jar,
    );

    // jetlinks-core: NO sources.jar, only .class fallback + transitive interface ConfigScopeSupport
    let config_meta_class = build_simple_classfile(
        "org/jetlinks/core/metadata/ConfigMetadata",
        "java/lang/Object",
        &["org/jetlinks/core/metadata/ConfigScopeSupport"],
        0x0601,
        None,
        &[],
        &[("getName", "()Ljava/lang/String;", None, None)],
    );
    let scope_support_class = build_simple_classfile(
        "org/jetlinks/core/metadata/ConfigScopeSupport",
        "java/lang/Object",
        &[],
        0x0601,
        None,
        &[],
        &[("getScope", "()Ljava/lang/String;", None, None)],
    );
    let jetlinks_jar = build_zip(&[
        (
            "org/jetlinks/core/metadata/ConfigMetadata.class",
            &config_meta_class,
            false,
        ),
        (
            "org/jetlinks/core/metadata/ConfigScopeSupport.class",
            &scope_support_class,
            true,
        ),
    ]);
    write_external_bytes(
        &h,
        "/mock-m2/org/jetlinks/jetlinks-core/1.3.2-SNAPSHOT/jetlinks-core-1.3.2-SNAPSHOT.jar",
        &jetlinks_jar,
    );

    let inv = h.invocation();
    let proj = inv.project().unwrap();
    let root = proj.root().unwrap();
    let mut classes = Vec::new();
    for path in proj.find_files("**/*.java", 100).unwrap() {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .trim_start_matches('/')
            .to_string();
        classes.extend(source::parse(&rel, &proj.read_file(&path).unwrap()));
    }

    let res = resolve_external_classes_with_roots(
        &proj,
        &root,
        &classes,
        &Scope::default(),
        &["/mock-m2", "/mock-gradle"],
    );
    assert_eq!(res.jar_count, 2);
    assert!(
        res.resolved_count >= 4,
        "resolved: {:?}",
        res.resolved_count
    );
    classes.extend(res.classes);

    let export = openapi::generate(&classes, &Scope::default(), "API");
    assert!(
        export.warnings.is_empty(),
        "warnings: {:?}",
        export.warnings
    );
    assert_eq!(
        export.endpoints.len(),
        2,
        "external @RestController must not add endpoints"
    );

    let schemas = &export.document["components"]["schemas"];
    assert_eq!(
        schemas["PagerResultDeviceEntity"]["properties"]["total"]["description"],
        "总数来自源码包"
    );
    assert_eq!(
        schemas["PagerResultDeviceEntity"]["properties"]["data"]["items"]["$ref"],
        "#/components/schemas/DeviceEntity"
    );
    assert_eq!(
        schemas["DeviceEntity"]["properties"]["id"]["type"],
        "string"
    );
    assert_eq!(
        schemas["DeviceEntity"]["properties"]["id"]["description"],
        "主键ID"
    );
    assert_eq!(
        schemas["ConfigMetadata"]["properties"]["name"]["type"],
        "string"
    );
    assert_eq!(
        schemas["ConfigMetadata"]["properties"]["scope"]["type"],
        "string"
    );
}

#[test]
fn external_resolver_degrades_gracefully_without_fs_read_capability() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
    ]);
    h.add_file(
        "src/main/java/demo/Api.java",
        r#"
package demo;
import org.hswebframework.web.api.crud.entity.PagerResult;
import org.springframework.web.bind.annotation.*;
@RestController
public class Api {
    @GetMapping("/p")
    public PagerResult<String> p() { return null; }
}
"#,
    );
    // Should not error even though fs.read is not granted
    let export = scan(&h.invocation(), "project").expect("scan succeeds without fs.read");
    assert!(
        export
            .warnings
            .iter()
            .any(|w| w.contains("无法解析类型 PagerResult<String>")),
        "{:?}",
        export.warnings
    );
    let _ = DEFAULT_MAX_ENTRY_BYTES;
    let _ = ApifoxHelper;
}

#[test]
fn external_resolver_supports_gradle_cache_layout() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "fs.read:/mock-gradle",
        "fs.write:/mock-gradle",
    ]);
    h.add_file(
        "build.gradle.kts",
        r#"
dependencies {
    implementation("com.acme:acme-api:2.1.0")
}
"#,
    );
    h.add_file(
        "src/main/java/app/DemoController.java",
        r#"
package app;
import com.acme.api.AcmeDto;
import org.springframework.web.bind.annotation.*;
@RestController
public class DemoController {
    @GetMapping("/acme")
    public AcmeDto get() { return null; }
}
"#,
    );
    let acme_class = build_simple_classfile(
        "com/acme/api/AcmeDto",
        "java/lang/Object",
        &[],
        0x0021,
        None,
        &[("code", "Ljava/lang/String;", None, None)],
        &[],
    );
    let acme_jar = build_zip(&[("com/acme/api/AcmeDto.class", &acme_class, false)]);
    write_external_bytes(
        &h,
        "/mock-gradle/com.acme/acme-api/2.1.0/abc123hash/acme-api-2.1.0.jar",
        &acme_jar,
    );

    let inv = h.invocation();
    let proj = inv.project().unwrap();
    let root = proj.root().unwrap();
    let mut classes = Vec::new();
    for path in proj.find_files("**/*.java", 100).unwrap() {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .trim_start_matches('/')
            .to_string();
        classes.extend(source::parse(&rel, &proj.read_file(&path).unwrap()));
    }
    let res = resolve_external_classes_with_roots(
        &proj,
        &root,
        &classes,
        &Scope::default(),
        &["/mock-gradle"],
    );
    assert_eq!(res.jar_count, 1);
    assert_eq!(res.resolved_count, 1);
    classes.extend(res.classes);
    let export = openapi::generate(&classes, &Scope::default(), "API");
    assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    assert_eq!(
        export.document["components"]["schemas"]["AcmeDto"]["properties"]["code"]["type"],
        "string"
    );
}

#[test]
fn external_resolver_finds_multiple_sibling_artifacts_in_same_group_without_exhausting_budget() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "fs.read:/mock-m2",
        "fs.write:/mock-m2",
    ]);
    h.add_file(
        "pom.xml",
        r#"<project>
            <dependencies>
                <dependency>
                    <groupId>org.hswebframework.web</groupId>
                    <artifactId>hsweb-commons-crud</artifactId>
                    <version>5.0.2-SNAPSHOT</version>
                </dependency>
                <dependency>
                    <groupId>ch.qos.logback</groupId>
                    <artifactId>logback-classic</artifactId>
                    <version>1.5.0</version>
                </dependency>
            </dependencies>
        </project>"#,
    );
    h.add_file(
        "src/main/java/demo/DeviceEntity.java",
        r#"
package demo;
import org.hswebframework.web.api.crud.entity.GenericEntity;
public class DeviceEntity extends GenericEntity<String> {
    private String name;
}
"#,
    );
    h.add_file(
        "src/main/java/demo/DeviceCrudController.java",
        r#"
package demo;
import org.hswebframework.web.crud.web.reactive.ReactiveServiceCrudController;
import org.springframework.web.bind.annotation.*;
@RestController
@RequestMapping("/device")
public class DeviceCrudController implements ReactiveServiceCrudController<DeviceEntity, String> {
}
"#,
    );
    h.add_file(
        "src/main/java/demo/MissingHelperController.java",
        r#"
package demo;
import com.missing.UnknownExternalType;
import org.springframework.web.bind.annotation.*;
@RestController
@RequestMapping("/helper")
public class MissingHelperController {
    @GetMapping("/x")
    public UnknownExternalType x() { return null; }
}
"#,
    );

    // Sibling 1: hsweb-commons-crud (contains ReactiveServiceCrudController)
    let crud_jar = build_zip(&[(
        "org/hswebframework/web/crud/web/reactive/ReactiveServiceCrudController.java",
        r#"package org.hswebframework.web.crud.web.reactive;
import org.hswebframework.web.api.crud.entity.PagerResult;
import org.hswebframework.web.api.crud.entity.QueryParamEntity;
import org.springframework.web.bind.annotation.*;
public interface ReactiveServiceCrudController<E, K> {
    @PostMapping("/_query")
    default PagerResult<E> query(@RequestBody QueryParamEntity param) { return null; }
}
"#
        .as_bytes(),
        false,
    )]);
    let crud_class = build_simple_classfile(
        "org/hswebframework/web/crud/web/reactive/ReactiveServiceCrudController",
        "java/lang/Object",
        &[],
        0x0601,
        Some("<E:Ljava/lang/Object;K:Ljava/lang/Object;>Ljava/lang/Object;"),
        &[],
        &[],
    );
    let crud_main = build_zip(&[(
        "org/hswebframework/web/crud/web/reactive/ReactiveServiceCrudController.class",
        &crud_class,
        false,
    )]);
    write_external_bytes(
        &h,
        "/mock-m2/org/hswebframework/web/hsweb-commons-crud/5.0.2-SNAPSHOT/hsweb-commons-crud-5.0.2-SNAPSHOT.jar",
        &crud_main,
    );
    write_external_bytes(
        &h,
        "/mock-m2/org/hswebframework/web/hsweb-commons-crud/5.0.2-SNAPSHOT/hsweb-commons-crud-5.0.2-SNAPSHOT-sources.jar",
        &crud_jar,
    );

    // Sibling 2 in the SAME group (org/hswebframework/web): hsweb-commons-api (NOT in pom.xml directly!)
    let api_sources = build_zip(&[
        (
            "org/hswebframework/web/api/crud/entity/GenericEntity.java",
            r#"package org.hswebframework.web.api.crud.entity;
public class GenericEntity<PK> {
    private PK id;
}
"#
            .as_bytes(),
            false,
        ),
        (
            "org/hswebframework/web/api/crud/entity/PagerResult.java",
            r#"package org.hswebframework.web.api.crud.entity;
import java.util.List;
public class PagerResult<E> {
    private int total;
    private List<E> data;
}
"#
            .as_bytes(),
            false,
        ),
        (
            "org/hswebframework/web/api/crud/entity/QueryParamEntity.java",
            r#"package org.hswebframework.web.api.crud.entity;
public class QueryParamEntity {
    private Integer pageIndex;
}
"#
            .as_bytes(),
            false,
        ),
    ]);
    let generic_class = build_simple_classfile(
        "org/hswebframework/web/api/crud/entity/GenericEntity",
        "java/lang/Object",
        &[],
        0x0021,
        Some("<PK:Ljava/lang/Object;>Ljava/lang/Object;"),
        &[],
        &[],
    );
    let pager_class = build_simple_classfile(
        "org/hswebframework/web/api/crud/entity/PagerResult",
        "java/lang/Object",
        &[],
        0x0021,
        Some("<E:Ljava/lang/Object;>Ljava/lang/Object;"),
        &[],
        &[],
    );
    let query_class = build_simple_classfile(
        "org/hswebframework/web/api/crud/entity/QueryParamEntity",
        "java/lang/Object",
        &[],
        0x0021,
        None,
        &[],
        &[],
    );
    let api_main = build_zip(&[
        (
            "org/hswebframework/web/api/crud/entity/GenericEntity.class",
            &generic_class,
            false,
        ),
        (
            "org/hswebframework/web/api/crud/entity/PagerResult.class",
            &pager_class,
            false,
        ),
        (
            "org/hswebframework/web/api/crud/entity/QueryParamEntity.class",
            &query_class,
            false,
        ),
    ]);
    write_external_bytes(
        &h,
        "/mock-m2/org/hswebframework/web/hsweb-commons-api/5.0.2-SNAPSHOT/hsweb-commons-api-5.0.2-SNAPSHOT.jar",
        &api_main,
    );
    write_external_bytes(
        &h,
        "/mock-m2/org/hswebframework/web/hsweb-commons-api/5.0.2-SNAPSHOT/hsweb-commons-api-5.0.2-SNAPSHOT-sources.jar",
        &api_sources,
    );

    // Unrelated logback jar in pom.xml must NOT be loaded for UnknownExternalType
    let logback_jar = build_zip(&[("ch/qos/logback/classic/Logger.class", b"dummy", false)]);
    write_external_bytes(
        &h,
        "/mock-m2/ch/qos/logback/logback-classic/1.5.0/logback-classic-1.5.0.jar",
        &logback_jar,
    );

    let inv = h.invocation();
    let proj = inv.project().unwrap();
    let root = proj.root().unwrap();
    let mut classes = Vec::new();
    for path in proj.find_files("**/*.java", 100).unwrap() {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .trim_start_matches('/')
            .to_string();
        classes.extend(source::parse(&rel, &proj.read_file(&path).unwrap()));
    }

    let res = resolve_external_classes_with_roots(
        &proj,
        &root,
        &classes,
        &Scope::default(),
        &["/mock-m2"],
    );
    // Only hsweb-commons-crud and hsweb-commons-api should be loaded, NOT logback
    assert_eq!(res.jar_count, 2, "resolved: {:?}", res.resolved_count);
    classes.extend(res.classes);

    let export = openapi::generate(&classes, &Scope::default(), "API");
    // Only UnknownExternalType should remain unresolved; GenericEntity, PagerResult, QueryParamEntity all resolved
    assert_eq!(export.warnings.len(), 1, "{:?}", export.warnings);
    assert!(export.warnings[0].contains("UnknownExternalType"));

    let schemas = &export.document["components"]["schemas"];
    assert_eq!(
        schemas["DeviceEntity"]["properties"]["id"]["type"],
        "string"
    );
    assert_eq!(
        schemas["PagerResultDeviceEntity"]["properties"]["data"]["items"]["$ref"],
        "#/components/schemas/DeviceEntity"
    );
    assert_eq!(
        schemas["QueryParamEntity"]["properties"]["pageIndex"]["type"],
        "integer"
    );
}

#[test]
fn external_resolver_handles_same_package_nested_enum_and_version_fallback() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "fs.read:/mock-m2",
        "fs.write:/mock-m2",
    ]);
    h.add_file(
        "pom.xml",
        r#"<project>
            <dependencies>
                <dependency>
                    <groupId>org.jetlinks.plugin</groupId>
                    <artifactId>plugin-core</artifactId>
                    <version>1.0.6-SNAPSHOT</version>
                </dependency>
                <dependency>
                    <groupId>org.jetlinks</groupId>
                    <artifactId>rule-engine-api</artifactId>
                    <version>1.3.2-SNAPSHOT</version>
                </dependency>
            </dependencies>
        </project>"#,
    );
    h.add_file(
        "src/main/java/demo/HybridController.java",
        r#"
package demo;
import org.jetlinks.plugin.core.Description;
import org.jetlinks.rule.engine.api.task.TaskSnapshot;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/hybrid")
public class HybridController {
    @GetMapping("/desc")
    public Description desc() { return null; }

    @GetMapping("/snapshot")
    public TaskSnapshot snapshot() { return null; }
}
"#,
    );

    // plugin-core: pom declares 1.0.6-SNAPSHOT, but local repo only has 1.0.5
    let desc_class = build_simple_classfile(
        "org/jetlinks/plugin/core/Description",
        "java/lang/Object",
        &[],
        0x0021,
        None,
        &[("id", "Ljava/lang/String;", None, None)],
        &[],
    );
    let plugin_core_jar = build_zip(&[(
        "org/jetlinks/plugin/core/Description.class",
        &desc_class,
        false,
    )]);
    write_external_bytes(
        &h,
        "/mock-m2/org/jetlinks/plugin/plugin-core/1.0.5/plugin-core-1.0.5.jar",
        &plugin_core_jar,
    );

    // rule-engine-api: TaskSnapshot references same-package nested Task.State without explicit import
    let snapshot_sources = build_zip(&[
        (
            "org/jetlinks/rule/engine/api/task/TaskSnapshot.java",
            r#"package org.jetlinks.rule.engine.api.task;
public class TaskSnapshot {
    private Task.State state;
}
"#
            .as_bytes(),
            false,
        ),
        (
            "org/jetlinks/rule/engine/api/task/Task.java",
            r#"package org.jetlinks.rule.engine.api.task;
public interface Task {
    enum State { running, shutdown }
}
"#
            .as_bytes(),
            false,
        ),
    ]);
    let snapshot_class = build_simple_classfile(
        "org/jetlinks/rule/engine/api/task/TaskSnapshot",
        "java/lang/Object",
        &[],
        0x0021,
        None,
        &[(
            "state",
            "Lorg/jetlinks/rule/engine/api/task/Task$State;",
            None,
            None,
        )],
        &[],
    );
    let task_state_class = build_simple_classfile(
        "org/jetlinks/rule/engine/api/task/Task$State",
        "java/lang/Enum",
        &[],
        0x4011,
        None,
        &[],
        &[],
    );
    let rule_main_jar = build_zip(&[
        (
            "org/jetlinks/rule/engine/api/task/TaskSnapshot.class",
            &snapshot_class,
            false,
        ),
        (
            "org/jetlinks/rule/engine/api/task/Task$State.class",
            &task_state_class,
            false,
        ),
    ]);
    write_external_bytes(
        &h,
        "/mock-m2/org/jetlinks/rule-engine-api/1.3.2-SNAPSHOT/rule-engine-api-1.3.2-SNAPSHOT.jar",
        &rule_main_jar,
    );
    write_external_bytes(
        &h,
        "/mock-m2/org/jetlinks/rule-engine-api/1.3.2-SNAPSHOT/rule-engine-api-1.3.2-SNAPSHOT-sources.jar",
        &snapshot_sources,
    );

    let inv = h.invocation();
    let proj = inv.project().unwrap();
    let root = proj.root().unwrap();
    let mut classes = Vec::new();
    for path in proj.find_files("**/*.java", 100).unwrap() {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .trim_start_matches('/')
            .to_string();
        classes.extend(source::parse(&rel, &proj.read_file(&path).unwrap()));
    }

    let res = resolve_external_classes_with_roots(
        &proj,
        &root,
        &classes,
        &Scope::default(),
        &["/mock-m2"],
    );
    assert_eq!(res.jar_count, 2);
    classes.extend(res.classes);

    let export = openapi::generate(&classes, &Scope::default(), "API");
    assert!(export.warnings.is_empty(), "{:?}", export.warnings);
    assert_eq!(
        export.document["components"]["schemas"]["Description"]["properties"]["id"]["type"],
        "string"
    );
    assert_eq!(
        export.document["components"]["schemas"]["State"]["enum"],
        serde_json::json!(["running", "shutdown"])
    );
}

#[test]
fn external_resolver_skips_workspace_modules_and_unmatched_jars_do_not_consume_budget() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "fs.read:/mock-m2",
        "fs.write:/mock-m2",
    ]);
    h.add_file(
        "pom.xml",
        r#"<project>
            <groupId>org.jetlinks.community</groupId>
            <artifactId>device-manager</artifactId>
            <version>2.0.0</version>
            <dependencies>
                <dependency>
                    <groupId>org.jetlinks</groupId>
                    <artifactId>jetlinks-core</artifactId>
                    <version>1.3.2-SNAPSHOT</version>
                </dependency>
            </dependencies>
        </project>"#,
    );
    h.add_file(
        "src/main/java/demo/Api.java",
        r#"
package demo;
import org.jetlinks.core.metadata.ConfigMetadata;
import org.springframework.web.bind.annotation.*;
@RestController
public class Api {
    @GetMapping("/cfg")
    public ConfigMetadata cfg() { return null; }
}
"#,
    );
    // Workspace module artifact in ~/.m2/repository should be skipped
    let ws_jar = build_zip(&[("org/jetlinks/community/Dummy.class", b"dummy", false)]);
    write_external_bytes(
        &h,
        "/mock-m2/org/jetlinks/community/device-manager/2.0.0/device-manager-2.0.0.jar",
        &ws_jar,
    );
    // Unmatched sibling artifact in org.jetlinks group must NOT count toward jar_count
    let other_jar = build_zip(&[("org/jetlinks/other/Other.class", b"dummy", false)]);
    write_external_bytes(
        &h,
        "/mock-m2/org/jetlinks/jetlinks-alpha/1.0.0/jetlinks-alpha-1.0.0.jar",
        &other_jar,
    );
    let cfg_class = build_simple_classfile(
        "org/jetlinks/core/metadata/ConfigMetadata",
        "java/lang/Object",
        &[],
        0x0601,
        None,
        &[],
        &[("getName", "()Ljava/lang/String;", None, None)],
    );
    let core_jar = build_zip(&[(
        "org/jetlinks/core/metadata/ConfigMetadata.class",
        &cfg_class,
        false,
    )]);
    write_external_bytes(
        &h,
        "/mock-m2/org/jetlinks/jetlinks-core/1.3.2-SNAPSHOT/jetlinks-core-1.3.2-SNAPSHOT.jar",
        &core_jar,
    );

    let inv = h.invocation();
    let proj = inv.project().unwrap();
    let root = proj.root().unwrap();
    let mut classes = Vec::new();
    for path in proj.find_files("**/*.java", 100).unwrap() {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .trim_start_matches('/')
            .to_string();
        classes.extend(source::parse(&rel, &proj.read_file(&path).unwrap()));
    }
    let res = resolve_external_classes_with_roots(
        &proj,
        &root,
        &classes,
        &Scope::default(),
        &["/mock-m2"],
    );
    assert_eq!(res.jar_count, 1);
    assert_eq!(res.resolved_count, 1);
}

#[test]
fn parse_javap_verbose_extracts_generics_annotations_interfaces_and_enums() {
    let javap_out = r#"
Classfile jar:file:///D:/custom-repo/org/hswebframework/web/hsweb-commons-api/5.0.2/hsweb-commons-api-5.0.2.jar!/org/hswebframework/web/api/crud/entity/PagerResult.class
  flags: (0x0021) ACC_PUBLIC, ACC_SUPER
  this_class: #8                          // org/hswebframework/web/api/crud/entity/PagerResult
  super_class: #47                        // java/lang/Object
{
  private int total;
    descriptor: I
    flags: (0x0002) ACC_PRIVATE
    RuntimeVisibleAnnotations:
      0: #71(#72=s#75)
        io.swagger.v3.oas.annotations.media.Schema(
          description="数据总量"
        )

  private java.util.List<E> data;
    descriptor: Ljava/util/List;
    flags: (0x0002) ACC_PRIVATE
    Signature: #77                          // Ljava/util/List<TE;>;
    RuntimeVisibleAnnotations:
      0: #71(#72=s#78)
        io.swagger.v3.oas.annotations.media.Schema(
          description="数据列表"
        )
}
Signature: #108                         // <E:Ljava/lang/Object;>Ljava/lang/Object;Ljava/io/Serializable;

Classfile jar:file:///D:/custom-repo/org/jetlinks/jetlinks-core/1.3.2/jetlinks-core-1.3.2.jar!/org/jetlinks/core/metadata/ConfigMetadata.class
  flags: (0x0601) ACC_PUBLIC, ACC_INTERFACE, ACC_ABSTRACT
  this_class: #2                          // org/jetlinks/core/metadata/ConfigMetadata
  super_class: #4                         // java/lang/Object
{
  public abstract java.lang.String getName();
    descriptor: ()Ljava/lang/String;
    flags: (0x0401) ACC_PUBLIC, ACC_ABSTRACT
}
Signature: #10                          // Ljava/lang/Object;Lorg/jetlinks/core/metadata/ConfigScopeSupport;

Classfile jar:file:///D:/custom-repo/org/jetlinks/rule-engine-api/1.3.2/rule-engine-api-1.3.2.jar!/org/jetlinks/rule/engine/api/task/Task$State.class
  flags: (0x4031) ACC_PUBLIC, ACC_FINAL, ACC_SUPER, ACC_ENUM
  this_class: #2                          // org/jetlinks/rule/engine/api/task/Task$State
  super_class: #4                         // java/lang/Enum
{
  public static final org.jetlinks.rule.engine.api.task.Task$State running;
    descriptor: Lorg/jetlinks/rule/engine/api/task/Task$State;
    flags: (0x4019) ACC_PUBLIC, ACC_STATIC, ACC_FINAL, ACC_ENUM

  public static final org.jetlinks.rule.engine.api.task.Task$State shutdown;
    descriptor: Lorg/jetlinks/rule/engine/api/task/Task$State;
    flags: (0x4019) ACC_PUBLIC, ACC_STATIC, ACC_FINAL, ACC_ENUM
}
"#;
    let parsed = parse_javap_verbose(javap_out);
    assert_eq!(parsed.len(), 3);
    assert_eq!(
        parsed[0].qualified(),
        "org.hswebframework.web.api.crud.entity.PagerResult"
    );
    assert_eq!(parsed[0].generics, vec!["E"]);
    assert_eq!(parsed[0].fields[1].ty, "java.util.List<E>");
    assert_eq!(parsed[0].fields[0].annotations[0].name, "Schema");

    assert!(parsed[1].is_interface);
    assert_eq!(
        parsed[1].bases,
        vec!["org.jetlinks.core.metadata.ConfigScopeSupport"]
    );
    assert_eq!(parsed[1].fields[0].name, "name");

    assert_eq!(
        parsed[2].qualified(),
        "org.jetlinks.rule.engine.api.task.Task.State"
    );
    assert_eq!(parsed[2].variants, vec!["running", "shutdown"]);
}

#[test]
fn external_resolver_uses_mvn_and_javap_fallback_for_custom_repo_and_caches_classpath() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "process:mvn",
        "process:javap",
    ]);
    h.add_file(
        "pom.xml",
        r#"<project>
            <groupId>com.example</groupId>
            <artifactId>demo</artifactId>
            <version>1.0.0</version>
        </project>"#,
    );
    h.add_file(
        "src/main/java/demo/CustomRepoController.java",
        r#"
package demo;
import com.custom.CustomPage;
import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping("/custom")
public class CustomRepoController {
    @GetMapping("/page")
    public CustomPage<ItemDto> page() { return null; }
}
"#,
    );
    h.add_file(
        "src/main/java/demo/ItemDto.java",
        "package demo; public class ItemDto { private String title; }",
    );

    let mvn_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mvn_calls_cb = mvn_calls.clone();
    h.on_process("mvn", move |_args, _| {
        mvn_calls_cb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ProcessOutput {
            status: Some(0),
            stdout: r"D:\custom-maven-repo\com\custom\custom-api\1.0.0\custom-api-1.0.0.jar"
                .to_string(),
            stderr: String::new(),
        })
    });
    h.on_process("javap", |args, _| {
        assert!(args.iter().any(|a| a == "com.custom.CustomPage"));
        Ok(ProcessOutput {
            status: Some(0),
            stdout: r#"
Classfile jar:file:///D:/custom-maven-repo/com/custom/custom-api/1.0.0/custom-api-1.0.0.jar!/com/custom/CustomPage.class
  flags: (0x0021) ACC_PUBLIC, ACC_SUPER
  this_class: #2                          // com/custom/CustomPage
  super_class: #4                         // java/lang/Object
{
  private java.util.List<T> records;
    descriptor: Ljava/util/List;
    flags: (0x0002) ACC_PRIVATE
    Signature: #7                           // Ljava/util/List<TT;>;
}
Signature: #9                           // <T:Ljava/lang/Object;>Ljava/lang/Object;
"#
            .to_string(),
            stderr: String::new(),
        })
    });

    let first = scan(&h.invocation(), "project").expect("first scan");
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);
    assert!(
        first
            .notices
            .iter()
            .any(|n| n.contains("Maven/javap") && n.contains("1 个外部类型")),
        "{:?}",
        first.notices
    );
    assert_eq!(
        first.document["components"]["schemas"]["CustomPageItemDto"]["properties"]["records"]["items"]
            ["$ref"],
        "#/components/schemas/ItemDto"
    );
    assert_eq!(mvn_calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Second scan should reuse cached classpath in plugin_dir without re-running mvn
    let second = scan(&h.invocation(), "project").expect("second scan");
    assert!(second.warnings.is_empty(), "{:?}", second.warnings);
    assert_eq!(mvn_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn mvn_fallback_uses_offline_flag_and_skips_when_local_jars_leave_only_speculative_or_interface_gaps()
 {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "fs.read:/mock-m2",
        "fs.write:/mock-m2",
        "process:mvn",
        "process:javap",
    ]);
    h.add_file(
        "pom.xml",
        r#"<project>
            <groupId>com.demo</groupId>
            <artifactId>app</artifactId>
            <version>1.0.0</version>
            <dependencies>
                <dependency>
                    <groupId>com.ext</groupId>
                    <artifactId>ext-models</artifactId>
                    <version>1.0.0</version>
                </dependency>
            </dependencies>
        </project>"#,
    );
    let jar_path = "/mock-m2/com/ext/ext-models/1.0.0/ext-models-1.0.0.jar";
    let class_bytes = build_simple_classfile(
        "com/ext/ExtDto",
        "java/lang/Object",
        &[],
        0x0021,
        None,
        &[("id", "Ljava/lang/String;", None, None)],
        &[],
    );
    let jar_bytes = build_zip(&[("com/ext/ExtDto.class", &class_bytes, false)]);
    h.add_external_file(jar_path, "");
    openidea_plugin::host::fs::write_bytes(jar_path, &jar_bytes).unwrap();

    // Regular DTO implements an unresolved external marker interface + wildcard import:
    // neither is an explicit unresolved DTO field, and local repo already resolved ExtDto.
    let dto_src = "package demo;\nimport com.missing.*;\nimport com.ext.ExtDto;\npublic class LocalDto implements UnresolvedMarker {\n    private ExtDto ext;\n}\n";
    let ctrl_src = "package demo;\n@RestController\npublic class DemoController {\n    @GetMapping(\"/demo\") public LocalDto demo() { return null; }\n}\n";
    h.add_file("src/main/java/demo/LocalDto.java", dto_src);
    h.add_file("src/main/java/demo/DemoController.java", ctrl_src);

    let mvn_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mvn_calls_cb = mvn_calls.clone();
    h.on_process("mvn", move |_args, _| {
        mvn_calls_cb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ProcessOutput {
            status: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    });

    let mut classes = source::parse("src/main/java/demo/LocalDto.java", dto_src);
    classes.extend(source::parse(
        "src/main/java/demo/DemoController.java",
        ctrl_src,
    ));
    let res = resolve_external_classes_with_roots(
        &h.invocation().project().unwrap(),
        "/project",
        &classes,
        &Scope::default(),
        &["/mock-m2"],
    );
    assert_eq!(res.jar_count, 1);
    assert_eq!(res.resolved_count, 1);
    assert_eq!(mvn_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn mvn_fallback_passes_offline_flag_when_invoked_and_respects_task_cancellation() {
    let h = MockHost::install().with_capabilities(&[
        "project.read",
        "project.write",
        "editor.read",
        "storage",
        "process:mvn",
        "process:javap",
    ]);
    h.add_file(
        "pom.xml",
        r#"<project>
            <groupId>com.demo</groupId>
            <artifactId>app-offline</artifactId>
            <version>1.0.0</version>
        </project>"#,
    );
    h.add_file(
        "src/main/java/demo/OfflineController.java",
        "package demo;\nimport com.custom.MissingPage;\n@RestController\npublic class OfflineController {\n    @GetMapping(\"/offline\") public MissingPage get() { return null; }\n}\n",
    );

    let saw_offline = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw_offline_cb = saw_offline.clone();
    h.on_process("mvn", move |args, _| {
        saw_offline_cb.store(
            args.iter().any(|a| a == "-o"),
            std::sync::atomic::Ordering::SeqCst,
        );
        Ok(ProcessOutput {
            status: Some(1),
            stdout: String::new(),
            stderr: String::new(),
        })
    });

    let _ = scan(&h.invocation(), "project").expect("scan completes even if mvn fails");
    assert!(saw_offline.load(std::sync::atomic::Ordering::SeqCst));

    // Pre-cancelled task returns Error::Cancelled from resolve_external_classes_with_task
    let task = openidea_plugin::host::begin_task("cancel-test", true);
    h.cancel_tasks();
    let classes = vec![apifox_helper_openidea::source::Class {
        name: "Ctrl".into(),
        package: "demo".into(),
        path: "src/Ctrl.java".into(),
        annotations: vec![apifox_helper_openidea::source::Annotation {
            name: "RestController".into(),
            ..Default::default()
        }],
        ..Default::default()
    }];
    let err = apifox_helper_openidea::external::resolve_external_classes_with_task(
        &h.invocation().project().unwrap(),
        "/project",
        &classes,
        &apifox_helper_openidea::openapi::Scope::default(),
        Some(&task),
    )
    .unwrap_err();
    assert!(matches!(err, openidea_plugin::Error::Cancelled));
}
