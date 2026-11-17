# Apifox Helper for OpenIDEA

[![CI](https://github.com/jhzhang09/apifox-helper-openidea/actions/workflows/ci.yml/badge.svg)](https://github.com/jhzhang09/apifox-helper-openidea/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/jhzhang09/apifox-helper-openidea)](https://github.com/jhzhang09/apifox-helper-openidea/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

从 Java / Kotlin 后端源码生成 OpenAPI，在 OpenIDEA 中预览、调试，并同步到 Apifox。

这是独立开发的 **OpenIDEA 社区插件**，与 Apifox、OpenIDEA 官方没有隶属关系。插件以 Rust 编写，运行在 OpenIDEA 的 WebAssembly 沙箱中，使用系统密钥库保存访问令牌。

**[下载最新插件](https://github.com/jhzhang09/apifox-helper-openidea/releases/latest)** · [配置与说明优先级](docs/configuration.md) · [兼容范围](docs/compatibility.md) · [开发与构建](docs/development.md) · [更新记录](CHANGELOG.md)

## 能做什么

- **源码生成文档**：识别 Spring MVC、OpenFeign、JAX-RS 常见注解，导出 OpenAPI 3.0.3 JSON / YAML，无需启动服务或构建后端项目。
- **保留真实数据模型**：解析嵌套 DTO、泛型、继承、集合、Map、内部类和循环引用；模型使用可读名称，不按 `Result` 等类名硬套响应结构。
- **说明自动回退**：Swagger / OpenAPI 注解中的非空说明优先，Javadoc / KDoc 兜底，支持请求体、参数和响应说明。
- **接口树**：按模块、包、Controller、接口组织，支持搜索、折叠和源码跳转；自动排除标准测试源码与生成目录。
- **Apifox 同步**：选择账号可访问的项目，配置目录、分支和模块，右键同步单个接口或整个分组。
- **HTTP 调试**：从模型生成 JSON 请求体，编辑地址、请求头和请求体后发送，查看状态码、响应头与响应内容。

例如下面的返回类型：

```java
public ResponseEntity<Result<Page<User>>> list()
```

如果源码定义 `Result.payload`、`Page.rows`、`User.address`，生成结构会保留 `payload → rows[] → address`，并通过 `$ref` 引用对应模型。原始泛型没有提供类型参数时保留开放结构并提示，无法解析的业务类型会明确报告。

## 兼容性

| 项目 | 要求 |
| --- | --- |
| OpenIDEA | 0.1.46 或更新版本，插件 API 1.9+ |
| 插件 ID | `jhzhang.apifox-helper` |
| 编译目标 | `wasm32-wasip2`，同一安装包用于 OpenIDEA 支持的平台 |
| 服务端源码 | Java；Kotlin 需要显式声明类型 |
| Apifox 服务 | 公有云 `api.apifox.com` |

本项目使用声明解析器，尚不等同于 IntelliJ PSI 或完整的 Java / Kotlin 编译器。外部依赖解析和框架支持有明确边界，使用前请查看 [兼容范围](docs/compatibility.md)。

## 安装

1. 从 [GitHub Releases](https://github.com/jhzhang09/apifox-helper-openidea/releases/latest) 下载 `.oiplugin` 和 `SHA256SUMS`，核对文件校验和。
2. 打开 **Settings → Plugins → ⋮ → Install Plugin from Package File…**，选择安装包。部分版本的菜单显示为 **Install Plugin from Disk…**。
3. 审核权限并启用插件；在 Find Action 中运行 **Apifox Helper: 打开接口侧栏**。

当前 GitHub 发布包是**未签名包**，需从磁盘安装并确认权限。公开仓库和 Release 不代表已通过 OpenIDEA 社区市场审核；市场上架还需要发布者签名及注册表审核。参见 [官方发布规范](https://openidea.sh/en/plugins/publishing/marketplace)。

## 第一次使用

### 离线扫描与导出

打开后端项目，点击侧栏 **扫描接口**。扫描固定覆盖整个项目的生产源码，优先使用已打开文件的未保存内容。

树形结果按以下层级展示：

```text
接口数
└── 模块
    └── 包
        └── Controller
            ├── 接口名称    GET /api/users
            └── 接口名称    POST /api/users
```

- **单击接口**：查看响应模型、OpenAPI 和调试请求。
- **双击接口**：跳转源码声明。
- **右键接口或分组**：上传到 Apifox，或导出 OpenAPI JSON / YAML；分组操作包含其全部下级接口，搜索只影响显示范围。
- **右键根节点**：操作整个扫描结果。

导出文件为项目根目录的 `apifox-openapi.json` / `apifox-openapi.yaml`。已有文件会询问是否覆盖。

### 配置 Apifox 同步

1. 在 Apifox 的账号设置中创建 **API 访问令牌**。
2. 在 **Settings → Plugins → Apifox Helper → Secrets** 中配置 `Apifox Access Token`，允许 `secrets` 和 `net:api.apifox.com`。
3. 打开侧栏 **项目配置 → 同步项目**。首次点开自动加载项目，可按项目名、团队或 ID 搜索；选中后弹层自动收起，也可以手动填写 ID。
4. 按需填写接口目录、模型目录、分支和模块 ID，点击 **保存项目配置**。
5. 扫描后右键目标接口或分组，选择 **上传到 Apifox**，核对同步确认框中的目标和覆盖策略。

默认策略为 `AUTO_MERGE`，不会主动删除未匹配资源或移动已有接口目录。具体合并结果由 Apifox 决定，已有手工维护内容仍应核对。未解析模型或重复路由等阻断错误需要先处理；结构提示可在侧栏展开查看。

切换同步项目会清空原项目的目录、分支和模块 ID，避免误用；选择后统一保存才会生效。配置按**本地项目根目录**隔离，移动项目或换工作树会使用另一份配置。

### 调试接口

选择接口后打开 **调试**，替换 URL 中的路径参数，检查请求头和请求体，再点击发送。非 GET / HEAD / OPTIONS 请求会显示确认提示。

当前支持文本与 JSON 请求体，不提供二进制文件上传、会话管理或前后置脚本。调试目标必须通过 OpenIDEA 的网络权限检查。

## 权限与数据流

| 权限 | 必需 | 用途 |
| --- | --- | --- |
| `project.read` | 是 | 读取项目源码、基础配置和 Maven 模块信息，定位声明 |
| `project.write` | 是 | 用户执行导出时写入 OpenAPI 文件 |
| `editor.read` | 是 | 读取光标位置和未保存的编辑缓冲区 |
| `storage` | 是 | 按本地项目根目录保存同步配置 |
| `secrets` | 否 | 检查访问令牌是否已设置，并由宿主注入请求头 |
| `net:api.apifox.com` | 否 | 用户选择项目时查询列表，执行同步时上传 OpenAPI |
| `net:localhost` / `net:127.0.0.1` | 否 | 本机 HTTP(S) 接口调试 |
| `net:ask` | 否 | 调试其他 HTTPS 主机时，由 OpenIDEA 按主机询问授权 |

离线生成文档无需网络或令牌。插件没有遥测、后台同步或任意进程执行权限。

- **项目查询**：向 Apifox 发出 GET 请求，返回团队和项目名称、ID。
- **文档同步**：上传生成的 OpenAPI，包含接口路径、数据模型、说明与示例；不上传 Java / Kotlin 源文件。
- **接口调试**：向用户填写的目标发送请求头和请求体。
- **访问令牌**：由 OpenIDEA 系统密钥库保管，插件请求使用 `{{secret:apifox_token}}` 占位符，宿主替换；不进入 HTML 页面、项目配置或导出文件。

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

构建、校验、发布流程见 [开发文档](docs/development.md)。示例源码位于 [examples/demo](examples/demo)，可在没有 IDE 的环境生成文档：

```sh
cargo run --locked --example export -- examples/demo > /tmp/example-openapi.json
cargo run --locked --example export -- examples/demo yaml > /tmp/example-openapi.yaml
```

## 问题反馈与贡献

请在 [Issues](https://github.com/jhzhang09/apifox-helper-openidea/issues) 提供插件/OpenIDEA 版本、最小复现和脱敏源码，参考 [贡献说明](CONTRIBUTING.md)。安全问题请使用 [安全报告渠道](SECURITY.md)。

## 许可与致谢

本项目采用 [Apache-2.0](LICENSE)，第三方声明见 [NOTICE](NOTICE)。

感谢 [OpenIDEA](https://github.com/l0ng-ai/openidea) 的插件 SDK 和示例、[Apifox Helper](https://plugins.jetbrains.com/plugin/20549-apifoxhelper) 的功能设计，以及 [spring-api-export](https://github.com/YUYUY9527/spring-api-export) 的参考实现。本项目独立实现适配逻辑，未复制 Apifox 官方插件或 spring-api-export 的源码，也不分发原始 IDEA 插件安装包。
