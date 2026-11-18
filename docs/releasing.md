# 正式发版与社区市场更新

[返回 README](../README.md) · [开发与构建](development.md)

本项目后续正式更新统一按本手册执行：验证源码 → 公开 CI 构建 → 使用原发布者密钥签名 → 发布 GitHub Release → 提交社区注册表更新 PR → 核验市场同步。

本手册与当前构建脚本、CI 和社区规则于 **2026-11-18** 核对。执行发布前重新确认官方规则及工具版本。用户明确要求“发布新版并更新市场”时，可连续完成该流程；仅修改代码、文档或打本地候选包不视为授权对外发布。

## 固定身份与发布原则

| 项目 | 值 |
| --- | --- |
| 源码与 Release | `jhzhang09/apifox-helper-openidea` |
| 插件 ID | `jhzhang.apifox-helper` |
| 发布者 | `jhzhang` |
| 维护者 GitHub 账号 | `jhzhang09` |
| 社区注册表 | `l0ng-ai/openidea-plugins` |
| 注册表 fork | `jhzhang09/openidea-plugins` |
| 插件条目 | `plugins/jhzhang.apifox-helper.toml` |
| 已注册发布者 | `publishers/jhzhang.toml` |
| 本机签名文件约定 | `~/.config/openidea-publishers/jhzhang.key` |

后续版本复用插件 ID 和原签名密钥，不重新注册发布者。私钥不写入仓库、日志、Release 或 PR；公钥以官方注册表条目为准。需要更换密钥时走官方的公钥轮换与背书流程。

每次正式发布使用新的版本号、标签和附件地址。已经发布的标签、安装包和 SHA256 保持不变；发现问题发布修复版本，不覆盖原附件。

**正式包直接使用对应提交的公开 CI 组件。** 本机构建用于开发和候选验证，不能代替公开 CI 的组件。签名之后再运行 `build.sh` 会覆盖同名安装包，必须避免。

## 1. 准备版本并验证

以下命令从源码仓库根目录开始。`0.1.14` 仅为示例，执行时换成实际的新版本。

```sh
APIFOX_VERSION=0.1.14
APIFOX_REPO=jhzhang09/apifox-helper-openidea
git status --short
gh auth status
```

检查当前 diff，保留无关改动。对齐 `Cargo.toml`、`plugin.toml` 的版本，更新 `plugin.toml` 的 change-notes 和 `CHANGELOG.md`。核对版本和标签尚未发布。

```sh
cargo check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
npm ci --ignore-scripts
npm run format:check
npm test
./scripts/build.sh
git diff --check
```

`cargo check` 同步根包版本到 Cargo.lock。工具链及示例生成见 [开发文档](development.md)。依赖变化时运行 `python3 scripts/third_party.py` 并核对许可变更。

修复行为应有相应回归。涉及界面或宿主生命周期时，按改动检查项目往返切换、关闭重开、未保存内容、扫描快照和上传目标。模拟网络测试不代表真实 Apifox 导入已验证；交付时明确验证边界。个人项目、令牌、真实导出和本地回归入口不能提交到公开仓库。

只暂存本次目标文件，包括必要的版本、文档、测试和预构建 WASM；审查暂存 diff 后提交并推送。若采用功能 PR，先将目标改动合入 main，再从实际合入提交发布。

```sh
git diff --cached --stat
git diff --cached --check
git commit -m "chore: 发布 v${APIFOX_VERSION}"
git push origin main
APIFOX_COMMIT=$(git rev-parse HEAD)
```

## 2. 取得对应提交的公开 CI 产物

```sh
gh run list --repo "$APIFOX_REPO" --workflow ci.yml --commit "$APIFOX_COMMIT"
```

选取 head SHA 完全匹配的运行，将其编号填入变量。确认 Rust、前端和构建各步骤实际执行并成功。

```sh
APIFOX_RUN_ID=填写运行编号
gh run view "$APIFOX_RUN_ID" --repo "$APIFOX_REPO" \
  --json headSha,status,conclusion,url
gh run watch "$APIFOX_RUN_ID" --repo "$APIFOX_REPO" --interval 10 --exit-status
APIFOX_CI_DIR=$(mktemp -d /tmp/apifox-ci.XXXXXX)
gh run download "$APIFOX_RUN_ID" --repo "$APIFOX_REPO" \
  --name apifox-helper-openidea --dir "$APIFOX_CI_DIR"
```

现有 CI 上传的是**未签名 `.oiplugin` 和 SHA256SUMS**。安装包内包含 `dist/plugin.wasm` 与 `dist/build-info.json`，不是独立的两个下载附件。产物保留期目前为 7 天，过期后需要重新运行对应提交的 CI。

`build-info.json` 记录源码提交、是否有未提交改动、运行编号、rustc -Vv（含 host）、编译目标与文件摘要。签名时保持 HEAD 为该提交，并保持发布输入文件不变；CI 通过后若又修改源码或文档，应提交并重新取得匹配的产物。

## 3. 签名并核验构建来源

使用官方 `openidea-plugin`，发布前检查其版本和当前 OpenIDEA 兼容性。v0.1.13 已使用官方 v0.1.47 工具验证；取得工具的方法见 [官方 Quickstart](https://openidea.sh/en/plugins/quickstart)。

```sh
openidea-plugin --version
./scripts/sign.sh "$HOME/.config/openidea-publishers/jhzhang.key" "$APIFOX_CI_DIR"
```

可通过 `OPENIDEA_PLUGIN` 指定官方工具路径。脚本校验来源仓库、运行编号、源码提交、干净构建状态及关键输入摘要，然后直接使用 CI 组件执行官方 `pack` 和 `check`，不重新编译，并更新校验和。不要在发布过程中重新生成密钥。

再次比较公开构建与签名包，而不是仅凭打包成功判断来源一致：

```sh
python3 - "$APIFOX_CI_DIR" <<'PY'
from pathlib import Path
import hashlib
import json
import subprocess
import sys
import tomllib
import zipfile

manifest = tomllib.loads(Path("plugin.toml").read_text())
name = f'{manifest["id"]}-{manifest["version"]}.oiplugin'
with zipfile.ZipFile(Path(sys.argv[1]) / name) as ci:
    component = ci.read("dist/plugin.wasm")
    info = json.loads(ci.read("dist/build-info.json"))
with zipfile.ZipFile(Path("dist") / name) as signed:
    assert signed.read("dist/plugin.wasm") == component
    assert json.loads(signed.read("SIGNATURE.json"))["publisher"] == "jhzhang"
assert info["source_commit"] == subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
assert info["source_dirty"] is False
assert info["sha256"]["dist/plugin.wasm"] == hashlib.sha256(component).hexdigest()
assert info["wasm_size"] == len(component)
print("CI 与签名包组件逐字节一致：", info["wasm_size"], info["sha256"]["dist/plugin.wasm"])
PY
```

公钥信任由注册表中的 `publishers/jhzhang.toml` 确定。可在隔离目录中用该条目的公钥创建索引骨架，运行 `openidea-plugin index <目录>` 复核；社区 CI 也会执行公钥与签名检查。私钥不会提供给注册表。

## 4. 发布正式 Release 并核验下载

在忽略的 `output/` 目录准备发布说明文件，例如 `output/release-notes.md`。说明应包含功能与修复、兼容版本、验证结果及边界、源码提交、公开 CI 链接、签名身份、组件大小和 SHA256。

```sh
git tag -a "v${APIFOX_VERSION}" "$APIFOX_COMMIT" -m "Apifox Helper for OpenIDEA v${APIFOX_VERSION}"
git push origin "v${APIFOX_VERSION}"
gh release create "v${APIFOX_VERSION}" --repo "$APIFOX_REPO" \
  --verify-tag --latest --title "v${APIFOX_VERSION}" \
  --notes-file output/release-notes.md \
  "dist/jhzhang.apifox-helper-${APIFOX_VERSION}.oiplugin" \
  dist/build-info.json dist/SHA256SUMS
```

Release 必须有三个附件：签名安装包、`build-info.json`、`SHA256SUMS`。`sign.sh` 已为前两个文件生成校验和。标签仍指向构建来源提交，不能为了补一个预构建二进制而改指向。

```sh
APIFOX_DOWNLOAD_DIR=$(mktemp -d /tmp/apifox-release-check.XXXXXX)
gh release download "v${APIFOX_VERSION}" --repo "$APIFOX_REPO" --dir "$APIFOX_DOWNLOAD_DIR"
(
  cd "$APIFOX_DOWNLOAD_DIR"
  shasum -a 256 -c SHA256SUMS
)
cmp "dist/jhzhang.apifox-helper-${APIFOX_VERSION}.oiplugin" \
  "$APIFOX_DOWNLOAD_DIR/jhzhang.apifox-helper-${APIFOX_VERSION}.oiplugin"
cmp dist/build-info.json "$APIFOX_DOWNLOAD_DIR/build-info.json"
```

将最终附件和 CI 证据归档到忽略的 `output/releases/v<版本>/`。签名会替换工作区的 `dist/plugin.wasm`；如需清理生成差异，先归档，再仅恢复本次生成的预构建文件到源码提交版本，不覆盖其他改动。

## 5. 向社区注册表提交新版 PR

**只发布 Release 不会自动更新市场。** 每次更新都需要修改注册表并提交新的 PR。已合并的首次收录 PR 不再追加新版本；尚未合并的同一发布申请可继续更新原分支。

在独立 checkout 中操作注册表，避免与插件源码仓库混淆：

```sh
APIFOX_REGISTRY_DIR=$(mktemp -d /tmp/apifox-registry.XXXXXX)
git clone https://github.com/jhzhang09/openidea-plugins.git "$APIFOX_REGISTRY_DIR"
cd "$APIFOX_REGISTRY_DIR"
git remote add upstream https://github.com/l0ng-ai/openidea-plugins.git
git fetch upstream main
git switch -c "update-apifox-helper-${APIFOX_VERSION}" upstream/main
```

在 `plugins/jhzhang.apifox-helper.toml` 中追加新版 `[[versions]]`，例如：

```toml
[[versions]]
version = "0.1.14"
package = "https://github.com/jhzhang09/apifox-helper-openidea/releases/download/v0.1.14/jhzhang.apifox-helper-0.1.14.oiplugin"
sha256 = "填写最终签名包的64位SHA256"
```

复制的是最终签名包的 SHA256，不是 WASM 或 CI 未签名包的摘要。保留现有 repository、suggest 及旧版本条目，特别是旧插件 API 的兼容版本；不为常规更新修改发布者公钥、owners 或其他插件。

准备更新 PR 说明文件，列出版本、Release/源码/CI 链接、变更、测试与官方检查结果。如回应审核，按条目附修复依据。界面变化可提供不含业务数据的演示；与同类插件比较时描述可核实的功能差异，不将已有菜单与弹窗称为“无界面”。

```sh
python3 -m unittest discover -s scripts -q
git diff --check
git add plugins/jhzhang.apifox-helper.toml
git commit -m "chore: 更新 jhzhang.apifox-helper 至 ${APIFOX_VERSION}"
python3 scripts/registry.py validate --base upstream/main --author jhzhang09
git diff upstream/main --stat
git push -u origin "update-apifox-helper-${APIFOX_VERSION}"
gh pr create --repo l0ng-ai/openidea-plugins --base main \
  --head "jhzhang09:update-apifox-helper-${APIFOX_VERSION}" \
  --title "更新 jhzhang.apifox-helper 至 ${APIFOX_VERSION}" \
  --body-file /实际路径/market-update.md
```

常规版本更新的 PR diff 应只有本插件条目。核实检查实际执行及结果；维护者要求修改时更新该未合并 PR，必要时发新的修复版本，不覆盖已有 Release。

## 6. 核验收录与市场同步

| 状态 | 可以确认的结果 |
| --- | --- |
| Release 已发布 | 用户可以从磁盘安装；还不能称新版已上架 |
| 注册表 PR 已提交、CI 通过 | 等待维护者审核合并 |
| PR 已合并 | 注册表变更已获接受，继续检查合并后的索引发布 |
| 社区 index.json 已包含新版 | 社区索引发布完成 |
| 官方 plugins-index.json 已包含新版 | 官方默认市场索引已同步 |
| IDE 刷新后可见新版 | 该 IDE 可发现并安装其 API 兼容的新版 |

在 GitHub 查看 PR 审核和 Checks，合并后确认注册表 main 的发布检查成功。然后分别读取：

- [社区 index.json](https://github.com/l0ng-ai/openidea-plugins/releases/download/index/index.json)。
- [官方默认市场 plugins-index.json](https://github.com/l0ng-ai/openidea/releases/latest/download/plugins-index.json)。

核对 `jhzhang.apifox-helper` 的版本、发布者及 API 要求。官方说明索引通常在一小时内合入默认市场；IDE 的本地检查可能更晚，可在 Settings → Plugins 的管理菜单执行 **Rescan Plugins**。看到 PR 合并不能代替实际索引核验；下载超时只能报告“未核实”，不能推断同步失败或成功。

## 异常与交付

| 情况 | 处理 |
| --- | --- |
| SSH 报 publickey 错误 | 核对 gh 账号与仓库权限；可用现有 gh 凭据助手通过 HTTPS 推送，不输出令牌 |
| OAuth 缺少 workflow scope | 不擅自扩大授权；现有 CI 已能携带组件及构建信息，无需为常规发版修改 Workflow |
| CI 产物缺失或签名输入不匹配 | 重新取得对应提交的成功构建；不跳过脚本校验，也不临时拿本机编译件替代 |
| 社区 CI 排队或维护者待审 | 保留可追溯链接并报告状态，不声称已合并或已同步 |
| 密钥丢失、泄露或需轮换 | 按官方撤销/轮换流程处理，不直接用新密钥替换发布包 |

交付时给出 Release 和新注册表 PR 链接、版本、测试与签名/组件一致性结果，以及实际确认到哪一级市场状态；未完成的验证明确说明。未经本次用户授权，不额外发布、创建更新 PR 或合并 PR。

## 官方依据

- [社区注册表规则与更新方式](https://github.com/l0ng-ai/openidea-plugins#readme)
- [官方打包与签名](https://openidea.sh/en/plugins/publishing/packaging)
- [官方市场发布与索引同步](https://openidea.sh/en/plugins/publishing/marketplace)
- [本项目 CI](../.github/workflows/ci.yml)、[构建脚本](../scripts/build.sh)、[签名脚本](../scripts/sign.sh)、[构建来源信息](../scripts/build_info.py)
