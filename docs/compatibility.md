# 兼容范围与已知限制

[返回 README](../README.md)

## 源码识别

| 范围 | 已支持 |
| --- | --- |
| Spring / OpenFeign | RestController、Controller、FeignClient、RequestMapping、Get/Post/Put/Delete/PatchMapping，类/方法路径和路径/方法数组 |
| JAX-RS | Path、GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS、Produces/Consumes |
| 请求参数 | RequestBody、RequestParam、PathVariable、RequestHeader、CookieValue、RequestPart、MultipartFile；JAX-RS 常见 Query/Path/Header/Cookie/FormParam |
| 服务端参数 | 忽略 RequestAttribute、Context、Servlet 请求/响应、Principal、Authentication、BindingResult 等 |
| DTO | 嵌套类型、静态内部类、record、泛型、继承、List/Set/数组/Map、枚举、循环引用、类型位置注解 |
| 包装类型 | 业务包装保留实际字段；解包 ResponseEntity、HttpEntity、Mono、Optional 等，Flux 按数组处理 |
| Kotlin | 显式类型的 data class 构造参数、属性、可空类型、显式返回类型的 suspend fun |
| 说明 | Javadoc、KDoc、Java /// 注释和常见 Swagger/OpenAPI 说明注解 |
| 字段 | JsonProperty、JSONField、SerializedName 命名，JsonIgnore 忽略，常见 Bean Validation 约束 |
| 基础库 | JDK Properties；Hutool Tree 5.8.x 默认节点结构；SseEmitter 与 text/event-stream |

Hutool Tree 保留 ID 泛型、递归 children 和开放的动态字段。可识别 TreeNodeConfig.DEFAULT_CONFIG 的字面量字段名 setter；局部定制、表达式键名、运行时扩展字段需要核对。

扫描排除标准的 src/test、src/testFixtures、src/integrationTest、src/integration-test、src/androidTest、src/testIntegration，以及根目录 test/tests 和生成目录。测试 DTO 和配置也被排除；生产源码包名中出现 test 不会被排除。

全项目扫描不依赖选中的目录或打开的标签；宿主 read_file 优先使用可用的未保存缓冲区，否则读取磁盘。标签恢复与内存文档加载不同步不会因此中断扫描。

扫描快照按本地项目根目录持久化，关闭工程或宿主重建页面后恢复上次结果，显示扫描时间并标记为缓存快照。没有快照的项目显示空结果。搜索、调试和配置草稿仍只在当前页面会话内隔离，不持久化调试请求或响应。上传始终重新扫描当前源码；原接口消失或产生解析错误时阻止上传。

面板通过窗口激活及每 750 ms 的轻量上下文检查识别项目切换；切换检测尚未完成时，后端也会拒绝旧项目动作，要求在当前项目重新操作。其他页面及过期请求的回复不会覆盖当前结果。

同名类型优先显式导入，再查同包类型和通配导入；真实歧义会报告错误。Spring 正则路由去除正则约束后，按最终 HTTP 方法和路径查重；无法无损表示的冲突保留首个声明并报告，上传会被阻止。

## 边界

这是词法和声明解析器，不是编译器或 IntelliJ PSI。以下能力尚不完整：

- Maven/Gradle 外部依赖 JAR 与完整 classpath 解析。
- Java 路由常量、任意表达式、注解处理器、自定义组合注解和 Controller 映射继承。
- Kotlin 类型推断、typealias、扩展属性。
- Jackson 自定义序列化、计算型 getter、多态与视图。
- Swagger 全部结构覆盖注解、原插件的完整高级规则和脚本。
- 完整 Spring profile / 环境变量 / 远端配置绑定。
- 独立数据模型导出、私有部署 Apifox 地址、目录/分支/模块在线选择。
- 文件二进制上传调试、Cookie 会话、前后置脚本和自动重试。

找不到业务类型时不会套用默认 code/msg/data 等结构，会报告未解析类型。原始泛型未提供 T、Map 动态键或 Tree 动态字段等使用开放结构并提示；应先检查预览再同步。

方法扫描范围目前依赖行号；同一行写多个接口时请使用侧栏单接口右键操作。未知注解和运行时动态行为无法保证全部发现。

## 验证范围

自动化验证包括 SDK MockHost 的命令与权限流程、模型与引用回归、说明优先级、项目列表协议、侧栏 DOM 交互、JSON/YAML 等价、OpenAPI 标准校验、Clippy 和 WASM 组件构建。

DOM 测试不等于原生 WebView 的真实尺寸验证。用户已在 OpenIDEA / Apifox 中试用并反馈过功能，但具体平台、IDE 版本和服务端配置仍应自行核对。自动化测试不连接真实 Apifox，不使用任何个人令牌。
