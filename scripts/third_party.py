#!/usr/bin/env python3
"""从锁定的 Cargo 依赖生成可公开的第三方声明和许可副本。"""
from pathlib import Path
import json
import subprocess

root = Path(__file__).resolve().parent.parent
metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], cwd=root
))
lines = ["# 第三方声明", "", "本项目使用 OpenIDEA Rust SDK（Apache-2.0），原始声明见 [vendor/openidea-plugin/NOTICE](vendor/openidea-plugin/NOTICE)。", "", "以下列出 Cargo.lock 对应的 Rust 构建、运行及测试依赖；范围可能多于最终 WASM 中实际使用的代码。许可文本保留原文。npm 依赖仅用于开发，不随插件包分发。", "", "| 依赖 | 版本 | 许可 | 许可文本 |", "| --- | --- | --- | --- |"]
packages = sorted((p for p in metadata["packages"] if p.get("source")), key=lambda p: (p["name"], p["version"]))
for package in packages:
    source = Path(package["manifest_path"]).parent
    candidates = set(source.glob("LICENSE*")) | set(source.glob("COPYING*"))
    if package.get("license_file"):
        candidates.add(source / package["license_file"])
    output = root / "licenses" / f'{package["name"]}-{package["version"]}'
    links = []
    for path in sorted(candidates):
        if not path.is_file():
            continue
        output.mkdir(parents=True, exist_ok=True)
        target = output / path.name
        target.write_bytes(path.read_bytes())
        links.append(f'[{path.name}]({target.relative_to(root).as_posix()})')
    if not links:
        raise SystemExit(f'依赖 {package["name"]} 缺少本地许可文本，请核对后再发布')
    lines.append(f'| {package["name"]} | {package["version"]} | {package.get("license") or "见许可文本"} | {", ".join(links)} |')
(root / "THIRD_PARTY_NOTICES.md").write_text("\n".join(lines) + "\n")
print(f"已生成 {len(packages)} 个 Rust 依赖的第三方声明")
