const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const crypto = require("node:crypto");
const { execFileSync, spawnSync } = require("node:child_process");

function rejectedArtifact(info, expected, zipped = false) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "apifox-sign-test-"));
  const original = fs.readFileSync("dist/plugin.wasm");
  try {
    fs.writeFileSync(path.join(dir, "build-info.json"), JSON.stringify(info));
    fs.writeFileSync(path.join(dir, "plugin.wasm"), "tampered-component");
    if (zipped) {
      execFileSync("python3", [
        "-c",
        "from pathlib import Path; import sys, zipfile; root=Path(sys.argv[1]); z=zipfile.ZipFile(root/'candidate.oiplugin', 'w'); z.write(root/'build-info.json', 'dist/build-info.json'); z.write(root/'plugin.wasm', 'dist/plugin.wasm'); z.close()",
        dir,
      ]);
      fs.unlinkSync(path.join(dir, "build-info.json"));
      fs.unlinkSync(path.join(dir, "plugin.wasm"));
    }
    const result = spawnSync(
      "bash",
      ["scripts/sign.sh", "/unused-test.key", dir],
      { encoding: "utf8" },
    );
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, expected);
    assert.deepEqual(fs.readFileSync("dist/plugin.wasm"), original);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test("签名拒绝其他提交的 CI 产物，且不会覆盖现有组件", () => {
  rejectedArtifact(
    { format: 1, source_commit: "different-commit", source_dirty: false },
    /源码提交不匹配/,
  );
});
test("签名从 CI 未签名包读取来源信息，也拒绝不匹配的源码提交", () => {
  rejectedArtifact(
    { format: 1, source_commit: "different-commit", source_dirty: false },
    /源码提交不匹配/,
    true,
  );
});

test("签名拒绝被篡改的 CI 组件，且不会读取私钥或覆盖现有组件", () => {
  const files = [
    "Cargo.toml",
    "Cargo.lock",
    "plugin.toml",
    "README.md",
    "NOTICE",
    "LICENSE",
    "icon.svg",
    "dist/plugin.wasm",
  ];
  const sha256 = Object.fromEntries(
    files.map((name) => [
      name,
      crypto.createHash("sha256").update(fs.readFileSync(name)).digest("hex"),
    ]),
  );
  rejectedArtifact(
    {
      format: 1,
      source_commit: execFileSync("git", ["rev-parse", "HEAD"], {
        encoding: "utf8",
      }).trim(),
      source_dirty: false,
      github_run_id: "synthetic-test",
      github_repository: "jhzhang09/apifox-helper-openidea",
      sha256,
    },
    /摘要不匹配：dist\/plugin.wasm/,
  );
});
