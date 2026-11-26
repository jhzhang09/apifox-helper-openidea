# Apifox Helper for OpenIDEA

[![CI](https://github.com/jhzhang09/apifox-helper-openidea/actions/workflows/ci.yml/badge.svg)](https://github.com/jhzhang09/apifox-helper-openidea/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/jhzhang09/apifox-helper-openidea)](https://github.com/jhzhang09/apifox-helper-openidea/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

从 Java / Kotlin 后端源码生成 OpenAPI，在 OpenIDEA 中预览、调试，并同步到 Apifox。

这是独立开发的 **OpenIDEA 社区插件**，与 Apifox、OpenIDEA 官方没有隶属关系。插件以 Rust 编写，运行在 OpenIDEA 的 WebAssembly 沙箱中，使用系统密钥库保存访问令牌。

**[下载最新插件](https://github.com/jhzhang09/apifox-helper-openidea/releases/latest)** · [配置与说明优先级](docs/configuration.md) · [兼容范围](docs/compatibility.md) · [开发与构建](docs/development.md) · [发布与市场更新](docs/releasing.md) · [更新记录](CHANGELOG.md)

## 能做什么

- **源码生成文档**：识别 Spring MVC、OpenFeign、JAX-RS 常见注解，支持控制器接口/泛型继承与跨类路由常量拼接，导出 OpenAPI 3.0.3 JSON / YAML，无需启动服务或构建后端项目。
- **保留真实数据模型**：解析嵌套 DTO、泛型、继承、集合、Map、内部类、枚举字典说明（`@JsonValue` / `x-apifox-enum`）、类级 `@JsonNaming`/`@JsonIgnoreProperties` 与循环引用；模型使用可读名称，不按 `Result` 等类名硬套响应结构。
- **说明自动回退**：Swagger / OpenAPI 注解中的非空说明优先，Javadoc / KDoc 兜底，支持请求参数、请求体、查询 DTO 和响应说明。
- **接口树与宿主集成**：按 Maven / Gradle 模块、包、Controller、接口组织，支持搜索、折叠、双击或诊断文件名跳转源码，并提供 OpenIDEA Endpoints 索引、编辑器行标记（Line Markers）和光标同步定位。
- **Apifox 同步**：选择账号可访问的项目，配置目录、分支和模块，右键或工具栏同步单个接口、整个分组或当前搜索筛选结果。
- **HTTP 调试**：从模型生成请求示例，支持路径/查询/请求头快捷填参、全局默认请求头、按接口隔离调试草稿与响应、一键复制 cURL / OpenAPI，以及响应 JSON 格式化与耗时显示。

例如下面的返回类型：

```java
public ResponseEntity<Result<Page<User>>> list()
```

如果源码定义 `Result.payload`、`Page.rows`、`User.address`，生成结构会保留 `payload → rows[] → address`，并通过 `$ref` 引用对应模型。原始泛型没有提供类型参数时保留开放结构并提示，无法解析的业务类型会明确报告。

## 可视化工作流

本插件以常驻侧栏组织“扫描 → 检查模型 → 调试 → 同步”，用户可以在上传前检查接口参数、请求体与响应模型，并按模块、Controller 或当前搜索结果操作。

界面上的主要能力包括：接口树搜索与分组右键操作、双击或诊断文件链接跳转源码、请求/响应模型树（含约束条件与枚举字典说明）展开、OpenAPI 关联模型预览与快捷复制、Apifox 项目搜索选择，以及按接口隔离草稿和响应的 HTTP 调试。扫描结果和配置按本地项目隔离；关闭工程后可恢复带时间标记的上次扫描，上传时重新读取源码校验。

与已发布的 [Spring Api Export v0.1.2](https://github.com/YUYUY9527/spring-api-export/tree/673d3a895bc14074034bd132097a629de24e31b6) 相比：

| 交互     | Spring Api Export v0.1.2             | 本插件                                    |
| -------- | ------------------------------------ | ----------------------------------------- |
| 入口     | 右键菜单、命令、宿主输入框与多选弹窗 | 常驻侧栏，同时保留命令入口                |
| 接口选择 | 弹窗接口列表                         | 模块/包/Controller 树、搜索、分组右键操作 |
| 文档检查 | 导出 Markdown / OpenAPI 文件         | 侧栏请求与响应模型树、OpenAPI 预览        |
| 调试     | 清单中未注册调试命令或视图           | 集成 HTTP 请求编辑、发送和响应查看        |

上述对比来自该版本的 [清单](https://github.com/YUYUY9527/spring-api-export/blob/673d3a895bc14074034bd132097a629de24e31b6/plugin.toml) 和 [命令实现](https://github.com/YUYUY9527/spring-api-export/blob/673d3a895bc14074034bd132097a629de24e31b6/src/lib.rs)。它已有菜单与弹窗界面；本插件的区别是常驻可视化工作台，并提供 Kotlin/JAX-RS 源码识别与调试流程。

## 兼容性

| 项目        | 要求                                                               |
| ----------- | ------------------------------------------------------------------ |
| OpenIDEA    | 0.1.46 或更新版本，插件 API 1.9+                                   |
| 插件 ID     | `jhzhang.apifox-helper`                                            |
| 编译目标    | `wasm32-wasip2`，同一安装包用于 OpenIDEA 支持的平台                |
| 服务端源码  | Java；Kotlin 需要显式声明类型                                      |
| Apifox 服务 | 默认公网 `api.apifox.com`，可按本地项目指定私有化 HTTPS API 根地址 |

本项目使用声明解析器，尚不等同于 IntelliJ PSI 或完整的 Java / Kotlin 编译器。外部依赖解析和框架支持有明确边界，使用前请查看 [兼容范围](docs/compatibility.md)。

## 安装

1. 从 [GitHub Releases](https://github.com/jhzhang09/apifox-helper-openidea/releases/latest) 下载 `.oiplugin` 和 `SHA256SUMS`，核对文件校验和。
2. 打开 **Settings → Plugins → ⋮ → Install Plugin from Package File…**，选择安装包。部分版本的菜单显示为 **Install Plugin from Disk…**。
3. 审核权限并启用插件；在 Find Action 中运行 **Apifox Helper: 打开接口侧栏**。

从 v0.1.12 起，正式 Release 使用发布者 **jhzhang** 的 Ed25519 密钥签名；v0.1.11 为未签名版本。社区市场收录以 [注册表](https://github.com/l0ng-ai/openidea-plugins) 合并及索引同步为准，提交审核期间可以从磁盘安装签名包。参见 [官方发布规范](https://openidea.sh/en/plugins/publishing/marketplace)。

## 第一次使用

### 离线扫描与导出

打开后端项目，点击侧栏 **扫描接口**。扫描固定覆盖整个项目的生产源码，优先使用已打开文件的未保存内容。

扫描不要求选中目录或提前打开源码文件；切换项目后仍按当前项目读取，恢复标签页未载入内存文档时可以读取磁盘内容。

完整扫描自动按本地项目保存最后一次结果和扫描时间，关闭工程后重开会恢复，并标记为缓存快照。上传前始终重新扫描当前源码并校验，缓存不会直接作为上传文档。快照保存失败会明确提示，详见 [扫描快照](docs/configuration.md#扫描快照)。

树形结果按以下层级展示：

```text
接口数
└── 模块
    └── 包
        └── Controller
            ├── 接口名称    GET /api/users
            └── 接口名称    POST /api/users
```

- **单击接口**：查看请求/响应模型、OpenAPI（含关联模型与工具栏操作）和调试请求。
- **生成测试数据**：在调试区按请求模型重新生成 JSON 请求体，并补全路径占位符和空查询参数。保留请求头与已填参数，不自动发送；生成值不是数据库中的真实业务记录。
- **双击接口或点击诊断文件名**：直接跳转源码声明（优先利用快照快速定位，不因无关文件语法错误阻塞）。
- **右键接口或分组**：上传到 Apifox，或导出 OpenAPI JSON / YAML；搜索筛选时分组菜单额外提供“仅同步/导出筛选接口”，单接口菜单还支持复制 cURL 与跳转源码。
- **右键根节点**：操作整个扫描结果。

导出文件为项目根目录的 `apifox-openapi.json` / `apifox-openapi.yaml`。已有文件会询问是否覆盖。

### 配置 Apifox 同步

1. 在 Apifox 的账号设置中创建 **API 访问令牌**。
2. 在 **Settings → Plugins → Apifox Helper → Secrets** 中配置 `Apifox Access Token`，允许 `secrets` 和 `net:api.apifox.com`。

   私有化部署：改为设置 `Apifox Private Access Token`，在侧栏 **Apifox 设置 → 私有化 API 地址** 填入管理员提供的 HTTPS 根地址（可带端口或反向代理前缀，不含接口路径），允许 `net:ask` 并授权目标主机。重新选择项目或手填项目 ID 后保存。留空切回公网；该地址不改变接口调试地址。私有化令牌是插件级密钥，多个私服不分别保存令牌；切换实例前请更换对应令牌。

3. 打开侧栏 **Apifox 设置 → 同步项目**。首次点开自动加载项目，可按项目名、团队或 ID 搜索；选中后弹层自动收起，也可以手动填写 ID。
4. 按需填写接口目录、模型目录、分支和模块 ID，点击 **保存项目配置**。调试地址和默认请求头在接口的 **调试 → 调试设置** 中单独保存。
5. 扫描后右键目标接口或分组，选择 **上传到 Apifox**，核对同步确认框中的目标和覆盖策略。

默认策略为 `AUTO_MERGE`，不会主动删除未匹配资源或移动已有接口目录。具体合并结果由 Apifox 决定，已有手工维护内容仍应核对。未解析模型或重复路由等阻断错误会自动展开，可在侧栏核对并点击文件名跳转修复。

切换同步项目会清空原项目的目录、分支和模块 ID，避免误用；选择后统一保存才会生效。配置按**本地项目根目录**隔离，移动项目或换工作树会使用另一份配置。

### 调试接口

选择接口后打开 **调试**（或通过命令在方法光标处直接聚焦调试视图），可通过快捷参数面板填写真实路径参数、查询参数和请求头，检查请求体与全局默认请求头后点击发送；未替换 `{param}` 路径占位符时会先提示定位，不会直接误发。非 GET / HEAD / OPTIONS 请求会显示确认提示；收到响应后会自动格式化 JSON 并显示请求耗时，各接口的调试草稿与响应互不串扰。

当前支持文本与 JSON 请求体，不提供二进制文件上传、会话管理或前后置脚本。调试目标必须通过 OpenIDEA 的网络权限检查。

## 权限与数据流

| 权限                              | 必需 | 用途                                                                        |
| --------------------------------- | ---- | --------------------------------------------------------------------------- |
| `project.read`                    | 是   | 读取项目源码、基础配置和 Maven / Gradle 模块信息，定位声明                  |
| `project.write`                   | 是   | 用户执行导出时写入 OpenAPI 文件                                             |
| `editor.read`                     | 是   | 读取光标位置和未保存的编辑缓冲区                                            |
| `storage`                         | 是   | 按本地项目根目录保存同步配置                                                |
| `secrets`                         | 否   | 检查访问令牌是否已设置，并由宿主注入请求头                                  |
| `clipboard`                       | 否   | 点击“复制 cURL / OpenAPI”时写入系统剪贴板                                   |
| `fs.read:~/.m2/repository`        | 否   | 按需读取本地 Maven 仓库依赖 JAR（`-sources.jar` / `.class`）以补全外部模型  |
| `fs.read:~/.gradle/caches`        | 否   | 按需读取本地 Gradle 缓存依赖 JAR（`-sources.jar` / `.class`）以补全外部模型 |
| `process:mvn`                     | 否   | 本地仓库未授权或自定义路径时，离线获取 Maven 依赖类路径（`mvn -o`）         |
| `process:javap`                   | 否   | 在获取类路径后按需读取外部类字节码结构（`javap -p -v`）                     |
| `net:api.apifox.com`              | 否   | 用户选择项目时查询列表，执行同步时上传 OpenAPI                              |
| `net:localhost` / `net:127.0.0.1` | 否   | 本机 HTTP(S) 接口调试                                                       |
| `net:ask`                         | 否   | 查询或同步私有化 Apifox、调试其他 HTTPS 主机时，按主机询问授权              |

离线生成文档无需网络或令牌；可选的 `fs.read` 与 `process:mvn` / `process:javap` 仅在用户显式授权后用于按需解析本地依赖类型，插件没有遥测、后台同步或任意进程执行权限。

- **项目查询**：向 Apifox 发出 GET 请求，返回团队和项目名称、ID。
- **文档同步**：上传生成的 OpenAPI，包含接口路径、数据模型、说明与示例；不上传 Java / Kotlin 源文件。
- **接口调试**：向用户填写的目标发送请求头和请求体。
- **访问令牌**：公网使用 `apifox_token`，私有化使用 `apifox_private_token`，均由宿主密钥库注入；不进入 HTML 页面、项目配置或导出文件。

远端明文 HTTP 需要显式声明并批准 `net.http:<host>`，当前安装包仅预设本机 HTTP 调试。更改清单后需要重新构建。各权限的定义以 [OpenIDEA 官方文档](https://openidea.sh/en/plugins/capabilities) 为准。

## 开发

需要 Rust 1.100.0、Node.js 24+、Python 3.11+：

```sh
rustup target add wasm32-wasip2
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
npm ci --ignore-scripts
npm test
./scripts/build.sh
```

构建输出为 `dist/plugin.wasm`、`.oiplugin` 和 `SHA256SUMS`。SDK 固定复制自 OpenIDEA `v0.1.46`，出处与许可保存在 [vendor/openidea-plugin/UPSTREAM.md](vendor/openidea-plugin/UPSTREAM.md)。

构建和本地校验见 [开发文档](docs/development.md)；正式发版及后续社区市场更新统一遵循 [发布手册](docs/releasing.md)。示例源码位于 [examples/demo](examples/demo)，可在没有 IDE 的环境生成文档：

```sh
cargo run --locked --example export -- examples/demo > /tmp/example-openapi.json
cargo run --locked --example export -- examples/demo yaml > /tmp/example-openapi.yaml
```

## 问题反馈与贡献

请在 [Issues](https://github.com/jhzhang09/apifox-helper-openidea/issues) 提供插件/OpenIDEA 版本、最小复现和脱敏源码，参考 [贡献说明](CONTRIBUTING.md)。安全问题请使用 [安全报告渠道](SECURITY.md)。

## 许可与致谢

本项目采用 [Apache-2.0](LICENSE)，完整第三方许可见 [NOTICE](NOTICE)，依赖清单见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

感谢 [OpenIDEA](https://github.com/l0ng-ai/openidea) 的插件 SDK 和示例、[Apifox Helper](https://plugins.jetbrains.com/plugin/20549-apifoxhelper) 的功能设计，以及 [spring-api-export](https://github.com/YUYUY9527/spring-api-export) 的参考实现。本项目独立实现适配逻辑，未复制 Apifox 官方插件或 spring-api-export 的源码，也不分发原始 IDEA 插件安装包。
