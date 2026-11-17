#!/usr/bin/env bash
# 私钥由调用方提供，脚本不复制或打印密钥内容。
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ $# -ne 1 ]]; then
    echo '用法: ./scripts/sign.sh <私钥绝对路径>' >&2
    exit 1
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
package.with_name("SHA256SUMS").write_text(f"{digest}  {package.name}\n")
print(f"已签名并校验 {package.name}；SHA256SUMS 已更新")
PY
