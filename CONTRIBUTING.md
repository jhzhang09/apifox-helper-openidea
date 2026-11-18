# 参与贡献

感谢提交问题和改进。

1. 先阅读 [开发文档](docs/development.md) 和 [兼容范围](docs/compatibility.md)。
2. 解析问题请提供能独立复现的最小 Java/Kotlin 代码、预期与实际 OpenAPI，以及插件和 OpenIDEA 版本。
3. 删除令牌、内部域名、个人信息及无法公开的业务内容。安全问题参见 [SECURITY.md](SECURITY.md)。
4. 修复时增加能复现问题的测试，保持现有接口契约和默认行为，避免无关重构。
5. 提交 PR 前运行 cargo test、Clippy、rustfmt 和 npm test，必要时重新生成示例与 WASM。

新增网络、文件写入、密钥或其他权限时，需要在 README 解释用途。请勿提交构建缓存、发布者私钥、个人 IDE 配置或真实项目导出。

正式发版和后续社区市场更新统一遵循 [发布手册](docs/releasing.md)。复用现有发布者和密钥，对公开 CI 组件签名，发布新的 Release，并新建注册表版本更新 PR；只发布 Release 不会自动更新市场。
