# 配置与文档说明

[返回 README](../README.md)

## 同步配置

| 配置 | 含义 |
| --- | --- |
| 同步项目 | 从账号项目列表选择，或手动输入项目 ID |
| 接口目录 / 模型目录 ID | 留空为项目根目录 |
| 分支 / 模块 ID | 留空为主分支和默认模块 |
| 已有文档处理 | AUTO_MERGE、OVERWRITE_EXISTING、KEEP_EXISTING、CREATE_NEW |
| 文档标题 | OpenAPI info.title |
| 调试服务地址 | 生成调试 URL 的基础地址，默认 http://localhost:8080 |

项目选择器首次打开自动查询，后续复用当前页面的列表。刷新、搜索、取消不会更改目标；选中后仍需保存。切换到其他项目会清空关联目录、分支和模块 ID，同一项目保留原配置。

查询失败时保留原 ID，可重试或手动填写。团队名称暂时无法加载时按团队 ID 显示并提示。接口目录、分支和模块仍需手动填写。

## 配置保存位置

配置由 OpenIDEA 的插件 storage 保存，键为 `config:<本地项目根目录>`。macOS 默认目录：

```text
~/Library/Application Support/ai.l0ng.openidea/plugins/.data/jhzhang.apifox-helper/storage/
```

文件名由宿主哈希生成。项目 ID、名称、目录、同步策略等配置与插件版本目录分离；同一插件 ID 的升级复用已有记录。不同根目录、工作树或移动后的项目使用独立配置，目前没有自动路径迁移。

页面渲染时直接回填保存值，重新打开配置面板时刷新；未保存输入不会被迟到回复覆盖。访问令牌单独保存在系统密钥库，不在此目录的项目配置中。

## 注解说明优先级

说明先去除首尾空白，空字符串和仅换行的值继续回退。模型、字段、请求体和响应说明分别取值，不互相覆盖。校验注解的 message 不替代字段说明。

| 内容 | 从高到低 |
| --- | --- |
| 模型说明 | Schema.description → ApiModel.description → 类 Javadoc |
| 模型标题 | Schema.title → ApiModel.value → 可读模型名；组件 ID 不随标题更改 |
| 字段说明 | Schema.description → ApiModelProperty.value → ApiModelProperty.notes → 字段 Javadoc |
| 接口名称 | Operation.summary → ApiOperation.value → 方法 Javadoc 首行 → 方法名 |
| 接口详细说明 | Operation.description → ApiOperation.notes → 方法 Javadoc 正文 |
| Controller 分组 | Tag.name → Api.tags 首个非空项 → Api.value → 类 Javadoc 首行 → 类名 |
| 参数说明 | 参数上的 Parameter.description → 按名称匹配的方法级 Parameter / Parameters / Operation.parameters → ApiParam.value → Schema.description → @param |
| 请求体说明 | 参数上的 Swagger RequestBody.description → 方法上的 Swagger RequestBody.description → Operation.requestBody.description → 参数说明规则 |
| 响应说明 | 按状态码匹配 ApiResponse.description 或旧版 ApiResponse.message；方法级先于类级，同一位置新版先于旧版；默认成功响应再回退到 @return / “成功” |

支持重复 ApiResponse、ApiResponses 容器、Operation.responses / requestBody / parameters 中的嵌套注解；Javadoc 的 @param / @return 支持多行。

Spring 与 Swagger 的同名 RequestBody 按全名、显式 import 或对应通配 import 区分。Swagger 的说明注解不改变 Spring 的绑定方式或 required 默认值。未求值的说明常量与表达式不覆盖 Javadoc。

本版本读取上述说明、标题与响应状态码，尚不使用注解 content/schema/implementation/ref 替换源码类型结构，也不解析远程引用。其他显式响应没有说明时使用“响应”，不为错误响应推测数据结构。

## 模型名称与顺序

- 普通模型使用简单类名，如 User、Address；泛型实例使用 ResultUser、ResultListUser。
- 内部按完整类型签名区分；不同包或泛型的简称发生冲突时使用固定签名后缀，避免覆盖。
- 嵌套对象、集合元素和循环引用使用 $ref。
- 父类字段先于当前类字段，其余按源码顺序导出，并生成 x-apifox-orders。
- 旧版编码模型名不会自动从 Apifox 删除。确认无引用后可手动清理旧模型。

## 基础配置与动态路由

`${key}` 从不带环境后缀的 application.yml、application.yaml、application.properties 中解析，支持嵌套键、YAML 多文档、默认值与递归引用。带 profile 激活条件的 YAML 文档和测试资源不参与。

例如：

```yaml
sse:
  path: /resource/sse
```

`${sse.path}/close` 会生成 `/resource/sse/close`。多个基础文件值冲突、循环引用、缺少配置或残留动态值会报告阻断错误。

这不是完整 Spring 配置绑定：不会读取进程环境、远端配置或推测运行时 profile，基础配置不代表实际部署的最终值。

## 旧规则

有限兼容 `.apifox-helper.properties`：

```properties
param.ignore=@org.springframework.web.bind.annotation.RequestAttribute
field.ignore=@example.InternalField
method.ignore=@example.InternalApi
```

只解释上述注解忽略规则，不执行 Groovy / JavaScript 等脚本；其他规则给出提示。RequestAttribute 等常见服务端参数已有内置忽略处理。

## 项目列表查询协议

项目与团队查询使用固定主机 api.apifox.com。协议依据官方 ApifoxHelper 2.1.2-262 安装包的资源配置，以及 VerifyRequest 的 javap 字节码反汇编核对，**不是原始源码**：

- GET /api/v1/user-projects；响应 data 数组含 id、name、teamId、type。
- GET /api/v1/user-teams?locale=zh-CN；响应 data 数组含 id、name。
- 请求头 X-Apifox-Version: 2024-03-28，Authorization 使用宿主令牌占位符。
- 仅提供 HTTP 类型项目。

这两个查询接口未列入当前公开开放 API 文档，作为官方插件兼容协议接入，可能随服务变化。结构不匹配或查询失败时保留原配置并提示手动填写。文档导入使用公开的 [OpenAPI 导入 API](https://apifox-openapi.apifox.cn/api-173409873)。
