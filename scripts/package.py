#!/usr/bin/env python3
"""将已构建的 WASM 组件打包成可从磁盘安装的 OpenIDEA 插件。"""
from pathlib import Path
import hashlib
import tomllib
import zipfile

root = Path(__file__).resolve().parent.parent
manifest = tomllib.loads((root / "plugin.toml").read_text())
files = ["plugin.toml", "dist/plugin.wasm", "icon.svg", "LICENSE", "NOTICE", "README.md",
         "vendor/openidea-plugin/LICENSE", "vendor/openidea-plugin/NOTICE"]
files.extend(["CHANGELOG.md", "THIRD_PARTY_NOTICES.md", "vendor/openidea-plugin/UPSTREAM.md"])
files.extend(path.relative_to(root).as_posix() for path in sorted((root / "docs").glob("*.md")))
files.extend(path.relative_to(root).as_posix() for path in sorted((root / "licenses").rglob("*")) if path.is_file())
cargo = tomllib.loads((root / "Cargo.toml").read_text())
if cargo["package"]["version"] != manifest["version"]:
    raise SystemExit("Cargo.toml 与 plugin.toml 版本不一致")
wasm = (root / manifest["wasm"]).read_bytes()
if wasm[:8] != b"\x00asm\x0d\x00\x01\x00":
    raise SystemExit("dist/plugin.wasm 必须是 WebAssembly Component，不能是 Core Module")
output = root / "dist" / f'{manifest["id"]}-{manifest["version"]}.oiplugin'
with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
    for relative in files:
        info = zipfile.ZipInfo(relative, date_time=(2026, 11, 17, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = 0o100644 << 16
        archive.writestr(info, (root / relative).read_bytes())
with zipfile.ZipFile(output) as archive:
    if archive.testzip() is not None:
        raise SystemExit("安装包完整性校验失败")
digest = hashlib.sha256(output.read_bytes()).hexdigest()
(root / "dist" / "SHA256SUMS").write_text(f"{digest}  {output.name}\n")
print(f"已生成 {output.name}（{output.stat().st_size} bytes）")
