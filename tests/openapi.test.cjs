const { test } = require("node:test");
const assert = require("node:assert/strict");
const Parser = require("@apidevtools/swagger-parser");
const fs = require("node:fs");
test("生成示例符合 OpenAPI 3.0，所有模型引用可解析", async () => {
  const document = JSON.parse(
    fs.readFileSync("dist/example-openapi.json", "utf8"),
  );
  const api = await Parser.validate(document);
  assert.equal(api.openapi, "3.0.3");
  const result =
    api.paths["/api/users"].get.responses["200"].content["application/json"]
      .schema;
  const user = result.properties.payload.allOf[0].properties.rows.items;
  assert.equal(
    user.properties.address.allOf[0].properties.point.allOf[0].properties
      .latitude.type,
    "number",
  );
  assert.equal(
    user.properties.contacts.additionalProperties.items.properties.city.type,
    "string",
  );
});
test("YAML 导出符合 OpenAPI，解析后的内容与 JSON 完全一致", async () => {
  const yaml = require("js-yaml");
  const source = fs.readFileSync("dist/example-openapi.yaml", "utf8");
  assert.match(source, /"openapi": "3.0.3"/);
  const document = yaml.load(source);
  assert.deepEqual(
    document,
    JSON.parse(fs.readFileSync("dist/example-openapi.json", "utf8")),
  );
  await Parser.validate(document);
});
