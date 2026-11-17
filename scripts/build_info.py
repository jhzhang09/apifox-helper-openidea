#!/usr/bin/env python3
"""记录发布组件的编译环境与输入摘要，默认路径映射只记录目标目录。"""
from pathlib import Path
import hashlib
import json
import os
import subprocess

root = Path(__file__).resolve().parent.parent


def command(*args):
    return subprocess.check_output(args, cwd=root, text=True, stderr=subprocess.DEVNULL).strip()


try:
    commit = command("git", "rev-parse", "HEAD")
    dirty = bool(command("git", "status", "--porcelain", "--untracked-files=normal", "--", ".", ":!dist/plugin.wasm"))
except (subprocess.CalledProcessError, FileNotFoundError):
    commit, dirty = None, True

files = ["Cargo.toml", "Cargo.lock", "plugin.toml", "README.md", "NOTICE", "LICENSE", "icon.svg", "dist/plugin.wasm"]
info = {
    "format": 1,
    "source_commit": commit,
    "source_dirty": dirty,
    "github_run_id": os.environ.get("GITHUB_RUN_ID"),
    "github_repository": os.environ.get("GITHUB_REPOSITORY"),
    "rustc_verbose": command("rustc", "-Vv"),
    "cargo_version": command("cargo", "-V"),
    "target": "wasm32-wasip2",
    "profile": "release",
    "path_remapping": {"workspace": "/workspace/apifox-helper-openidea", "cargo_home": "/cargo", "sysroot": "/rust"},
    "extra_rustflags": os.environ.get("APIFOX_BUILD_EXTRA_RUSTFLAGS", ""),
    "wasm_size": (root / "dist/plugin.wasm").stat().st_size,
    "sha256": {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in files},
}
(root / "dist/build-info.json").write_text(json.dumps(info, ensure_ascii=False, indent=2) + "\n")
print("已记录组件摘要与编译环境：dist/build-info.json")
