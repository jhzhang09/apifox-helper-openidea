#!/usr/bin/env bash
# 私钥由调用方提供，脚本不复制或打印密钥内容。
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# -lt 1 || $# -gt 2 ]]; then
    echo '用法: ./scripts/sign.sh <私钥绝对路径> [下载的 CI 产物目录]' >&2
    exit 1
fi
if [[ $# -eq 2 ]]; then
    python3 - "$2" <<'PY'
from pathlib import Path
import hashlib
import json
import subprocess
import sys
import zipfile

artifact = Path(sys.argv[1]).resolve()
if (artifact / "build-info.json").is_file():
    info_bytes = (artifact / "build-info.json").read_bytes()
    component = (artifact / "plugin.wasm").read_bytes()
else:
    packages = list(artifact.glob("*.oiplugin"))
    if len(packages) != 1:
        raise SystemExit("CI 产物目录必须包含唯一的未签名安装包；已拒绝签名")
    with zipfile.ZipFile(packages[0]) as package:
        info_bytes = package.read("dist/build-info.json")
        component = package.read("dist/plugin.wasm")
info = json.loads(info_bytes)
commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
if info.get("format") != 1 or info.get("source_commit") != commit or info.get("source_dirty") is not False:
    raise SystemExit("CI 产物的源码提交不匹配，或构建源码不干净；已拒绝签名")
if info.get("github_repository") != "jhzhang09/apifox-helper-openidea" or not info.get("github_run_id"):
    raise SystemExit("缺少本项目的公开 CI 来源；已拒绝签名")
files = ["Cargo.toml", "Cargo.lock", "plugin.toml", "README.md", "NOTICE", "LICENSE", "icon.svg", "dist/plugin.wasm"]
for name in files:
    data = component if name == "dist/plugin.wasm" else Path(name).read_bytes()
    if hashlib.sha256(data).hexdigest() != info.get("sha256", {}).get(name):
        raise SystemExit(f"CI 输入摘要不匹配：{name}；已拒绝签名")
Path("dist").mkdir(exist_ok=True)
Path("dist/plugin.wasm").write_bytes(component)
Path("dist/build-info.json").write_bytes(info_bytes)
print(f"已采用公开 CI #{info['github_run_id']} 的组件，未重新编译")
PY
fi
apifox_tool=${OPENIDEA_PLUGIN:-openidea-plugin}
apifox_package=$(python3 -c 'import tomllib; m=tomllib.load(open("plugin.toml", "rb")); print("dist/" + m["id"] + "-" + m["version"] + ".oiplugin")')
"$apifox_tool" pack . --key "$1" --publisher jhzhang --out "$apifox_package"
"$apifox_tool" check "$apifox_package"
python3 - "$apifox_package" <<'PY'
import hashlib
from pathlib import Path
import sys

package = Path(sys.argv[1])
digest = hashlib.sha256(package.read_bytes()).hexdigest()
checksums = f"{digest}  {package.name}\n"
info = package.with_name("build-info.json")
if info.is_file():
    checksums += f"{hashlib.sha256(info.read_bytes()).hexdigest()}  {info.name}\n"
package.with_name("SHA256SUMS").write_text(checksums)
print(f"已签名并校验 {package.name}；SHA256SUMS 已更新")
PY
