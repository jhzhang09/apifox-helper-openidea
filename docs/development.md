# 开发、验证与发布

[返回 README](../README.md)

## 工具链

- Rust 1.100.0，rustfmt、Clippy、wasm32-wasip2 target。
- Node.js 24+，用于侧栏 DOM、OpenAPI 和 YAML 测试。
- Python 3.11+，用于打包及第三方声明生成。
- 可选 wasm-tools，用于 WebAssembly Component 格式校验。
- 可选匹配版本的 openidea-plugin，用于宿主安装校验和开发链接。

SDK 复制自 OpenIDEA v0.1.46，位于 vendor/openidea-plugin，包含原始源码、WIT、LICENSE、NOTICE 和出处说明。升级 SDK 时应同步核对 API、清单和实际 IDE 的兼容性。

## 本地验证

```sh
rustup toolchain install 1.100.0 --profile minimal --component rustfmt --component clippy
rustup target add wasm32-wasip2 --toolchain 1.100.0
cargo +1.100.0 test --locked
cargo +1.100.0 clippy --locked --all-targets -- -D warnings
cargo +1.100.0 fmt --check
npm ci --ignore-scripts
npm run format:check
npm test
```

侧栏测试使用 JSDOM，不访问实际 Apifox。Rust 测试使用 SDK MockHost，验证参数、模型、权限、项目列表及配置流程。原生 WebView 的尺寸、焦点和实际服务端导入结果仍需在 IDE 中核对。

示例是解析样例，不是可运行的 Spring Boot 服务：

```sh
cargo run --locked --example export -- examples/demo > /tmp/example-openapi.json
cargo run --locked --example export -- examples/demo yaml > /tmp/example-openapi.yaml
```

提交示例更新前，可以将上述输出与 dist/example-openapi.json、dist/example-openapi.yaml 对比。

## 构建安装包

使用已选择的 Rust 工具链：

```sh
./scripts/build.sh
```

脚本构建 release WASM、复制到清单指定的 dist/plugin.wasm，并生成：

```text
dist/jhzhang.apifox-helper-<version>.oiplugin
dist/SHA256SUMS
dist/build-info.json
```

已安装 wasm-tools 时，构建脚本会额外运行组件格式校验。编译路径会重映射为通用目录，避免将构建机的本地用户名和绝对路径写入发布组件。这不保证不同平台或工具链产生逐字节相同的 WASM。

build-info.json 记录 rustc -Vv（含 host）、Cargo 版本、目标、附加编译选项、源码提交、源码是否干净、CI 运行编号、组件大小及关键文件 SHA256。源码包不带 Git 信息时标为非干净来源，不能用于正式 CI 签名。

如果已安装匹配版本的官方工具：

```sh
openidea-plugin check .
openidea-plugin check dist/jhzhang.apifox-helper-<version>.oiplugin
openidea-plugin link .
```

可通过官方文档中的命令获取对应版本工具：

```sh
cargo install --locked --git https://github.com/l0ng-ai/openidea --tag v0.1.46 ide-host --bin openidea-plugin
```

详见 [OpenIDEA Quickstart](https://openidea.sh/en/plugins/quickstart) 和 [Packaging and signing](https://openidea.sh/en/plugins/publishing/packaging)。本仓库自己的打包脚本验证组件头、文件完整性和版本，不能替代宿主的 openidea-plugin check。

## 第三方许可

Cargo.lock 固定依赖版本。更新依赖后运行：

```sh
python3 scripts/third_party.py
```

该脚本从本地 Cargo metadata 和依赖源码生成 THIRD_PARTY_NOTICES.md、licenses/ 及 NOTICE，不下载业务数据。NOTICE 汇总 SDK 声明和完整依赖许可，因为官方打包工具会收录 NOTICE，但不会自动收录 licenses/。npm 依赖只用于开发，不进入插件安装包。

## 正式发布与社区市场

正式发版及后续更新统一遵循 [发布手册](releasing.md)，该手册是本项目发布流程的唯一入口，包含验证、公开 CI 产物签名、Release 附件核验、社区注册表新版 PR 及市场同步检查。

本地 `build.sh` 和 CI 生成未签名开发包。正式 Release 使用发布者 `jhzhang` 的原 Ed25519 密钥，对对应提交的公开 CI 组件签名，不重新编译。仅发布 GitHub Release 不会自动更新社区市场；后续版本需要新的注册表更新 PR。

## 目录

| 路径 | 用途 |
| --- | --- |
| src/source.rs | Java/Kotlin 词法、声明、注解与 Javadoc 解析 |
| src/schema.rs | 类型定位、泛型替换和递归模型 |
| src/openapi.rs | 路由、参数、请求体、响应组装 |
| src/documentation.rs | 注解说明优先级及回退规则 |
| src/project.rs | 基础配置、占位符与 Tree 字段配置 |
| src/apifox.rs | 项目查询和公开导入协议 |
| src/plugin.rs / src/panel.html | 宿主命令、侧栏、配置、调试 |
| tests/ | 不依赖个人项目的回归测试 |
| examples/ | 通用源码解析样例 |
| vendor/ | 固定版本 SDK 与原始许可 |
