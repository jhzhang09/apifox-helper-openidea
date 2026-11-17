const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const { JSDOM } = require("jsdom");
const html = fs.readFileSync("src/panel.html", "utf8");
function panel(initial) {
  const sent = [];
  let receive;
  const source = initial
    ? html.replace(
        '"__APIFOX_CONFIG__"',
        JSON.stringify(initial).replaceAll("<", "\\u003c"),
      )
    : html;
  const dom = new JSDOM(source, {
    runScripts: "dangerously",
    beforeParse(w) {
      w.openidea = {
        postMessage(s) {
          sent.push(JSON.parse(s));
        },
        onMessage(fn) {
          receive = fn;
        },
      };
    },
  });
  return {
    dom,
    window: dom.window,
    $: (id) => dom.window.document.getElementById(id),
    sent,
    reply: (v) => receive(JSON.stringify(v)),
  };
}
const operation = {
  operationId: "stable-users-get",
  summary: "用户详情",
  parameters: [
    { name: "id", in: "query", schema: { type: "integer", default: 1 } },
  ],
  responses: {
    200: {
      description: "成功",
      content: {
        "application/json": { schema: { $ref: "#/components/schemas/Result" } },
      },
    },
  },
  requestBody: {
    content: {
      "application/json": { schema: { $ref: "#/components/schemas/User" } },
    },
  },
};
const doc = {
  components: {
    schemas: {
      Result: {
        type: "object",
        properties: { payload: { $ref: "#/components/schemas/User" } },
      },
      User: {
        type: "object",
        properties: {
          name: { type: "string" },
          address: { $ref: "#/components/schemas/Address" },
          parent: { $ref: "#/components/schemas/User" },
        },
      },
      Address: { type: "object", properties: { city: { type: "string" } } },
    },
  },
};
const exp = {
  document: doc,
  warnings: [],
  endpoints: [
    {
      method: "GET",
      path: "/users",
      file: "Api.java",
      line: 3,
      handler: "Api.get",
      operation,
    },
  ],
};
test("模型预览展开嵌套对象并终止循环引用", () => {
  const p = panel();
  p.reply({ op: "scan", export: exp });
  assert.match(p.$("model").textContent, /payload/);
  assert.match(p.$("model").textContent, /address/);
  assert.match(p.$("model").textContent, /city/);
  assert.match(p.$("model").textContent, /↻/);
  p.dom.window.close();
});
test("根据模型生成请求体，选择稳定标识并传递扫描范围", () => {
  const p = panel();
  p.reply({
    op: "config",
    config: { base_url: "http://localhost:8080" },
    tokenSet: false,
  });
  p.$("scan").click();
  p.reply({ op: "scan", export: exp });
  assert.equal(JSON.parse(p.$("body").value).address.city, "");
  assert.equal(JSON.parse(p.$("body").value).parent, null);
  context(p, p.$("list").querySelector("[data-endpoint]"));
  action(p, "upload");
  assert.deepEqual(p.sent.at(-1), {
    op: "upload",
    scope: "project",
    selected: ["stable-users-get"],
  });
  p.dom.window.close();
});
test("扫描固定整个项目，没有范围选择框，重新扫描关闭旧菜单", () => {
  const p = panel();
  assert.equal(p.$("scope"), null);
  assert.equal(p.$("scan").parentElement, p.$("search").parentElement);
  p.$("scan").click();
  assert.deepEqual(p.sent.at(-1), { op: "scan", scope: "project" });
  assert.equal(p.$("scan").disabled, true);
  p.reply({ op: "error", message: "源码未闭合" });
  assert.equal(p.$("scan").disabled, false);
  p.reply({ op: "scan", export: exp });
  context(p, p.$("list").querySelector("[data-endpoint]"));
  p.$("scan").click();
  assert.equal(p.$("context-menu").classList.contains("hidden"), true);
  assert.equal(p.sent.at(-1).scope, "project");
  p.dom.window.close();
});
test("接口名称以文本渲染，防止源码注释注入 HTML", () => {
  const p = panel();
  const e = JSON.parse(JSON.stringify(exp));
  e.endpoints[0].operation.summary =
    '<img src=x onerror="window.injected=true">';
  p.reply({ op: "scan", export: e });
  assert.equal(p.window.document.querySelectorAll("img").length, 0);
  assert.equal(p.window.injected, undefined);
  p.dom.window.close();
});
test("结构提示折叠展示，不把全部信息堆入错误栏", () => {
  const p = panel();
  const e = JSON.parse(JSON.stringify(exp));
  e.notices = [
    "R 未指定泛型参数 T，保留开放结构",
    "AjaxResult 动态字段按 additionalProperties 保留",
  ];
  p.reply({ op: "scan", export: e });
  assert.equal(p.$("diagnostics").open, false);
  assert.equal(p.$("diagnostics").classList.contains("hidden"), false);
  assert.match(p.$("diagnostic-summary").textContent, /2 条结构提示/);
  assert.equal(p.$("status").classList.contains("error"), false);
  assert.doesNotMatch(p.$("status").textContent, /additionalProperties/);
  assert.equal(p.$("diagnostic-list").children.length, 2);
  p.dom.window.close();
});
function treeExport() {
  const definitions = [
    [
      "lbs",
      "sample-api",
      "com.example.api.controller",
      "UserController",
      "用户管理",
      "get-user",
      "GET",
      "/users",
      "查询用户",
    ],
    [
      "lbs",
      "sample-api",
      "com.example.api.controller",
      "UserController",
      "用户管理",
      "create-user",
      "POST",
      "/users",
      "创建用户",
    ],
    [
      "lbs",
      "sample-api",
      "com.example.api.controller",
      "TagController",
      "标签管理",
      "get-tags",
      "GET",
      "/tags",
      "标签列表",
    ],
    [
      "admin",
      "sample-server",
      "com.example.admin.controller",
      "UserController",
      "用户管理",
      "admin-user",
      "GET",
      "/admin/users",
      "管理员用户",
    ],
  ];
  return {
    document: doc,
    warnings: [],
    notices: [],
    endpoints: definitions.map(
      ([
        modulePath,
        module,
        packageName,
        controller,
        controllerName,
        id,
        method,
        path,
        summary,
      ]) => ({
        module_path: modulePath,
        module,
        package: packageName,
        controller,
        controller_name: controllerName,
        method,
        path,
        file: `${modulePath}/src/main/java/${packageName.replaceAll(".", "/")}/${controller}.java`,
        line: 1,
        handler: controller + ".handler",
        operation: { ...operation, operationId: id, summary },
      }),
    ),
  };
}
const groups = (p) => [...p.$("list").querySelectorAll(".api-tree-node")];
const findGroup = (p, label, ancestor) =>
  groups(p).find(
    (n) =>
      n.querySelector(":scope > summary .tree-label").textContent === label &&
      (!ancestor || n.dataset.nodeKey.includes(ancestor)),
  );
function context(p, element) {
  element.dispatchEvent(
    new p.window.MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      clientX: 50,
      clientY: 60,
    }),
  );
}
function action(p, name) {
  p.$("context-menu").querySelector(`[data-action="${name}"]`).click();
}
test("按模块、压缩包名和 Controller 分组并显示准确数量", () => {
  const p = panel();
  p.reply({ op: "scan", export: treeExport() });
  assert.ok(findGroup(p, "sample-api"));
  assert.ok(findGroup(p, "sample-server"));
  assert.ok(findGroup(p, "com.example.api.controller"));
  assert.equal(
    findGroup(p, "sample-api").querySelector(":scope > summary .tree-count")
      .textContent,
    "(3)",
  );
  assert.equal(
    groups(p).filter(
      (n) =>
        n.querySelector(":scope > summary .tree-label").textContent ===
        "用户管理",
    ).length,
    2,
  );
  assert.equal(findGroup(p, "sample-api").open, false);
  assert.match(
    p.$("list").querySelector(".tree-columns").textContent,
    /名称路径/,
  );
  p.dom.window.close();
});
test("右键文件夹上传其全部接口，不包含同名的其他模块 Controller", () => {
  const p = panel();
  p.$("scan").click();
  p.reply({ op: "scan", export: treeExport() });
  const group = findGroup(p, "用户管理", "module:lbs");
  context(p, group.querySelector(":scope > summary"));
  assert.match(p.$("context-label").textContent, /用户管理（2 个接口）/);
  action(p, "upload");
  assert.deepEqual(p.sent.at(-1), {
    op: "upload",
    scope: "project",
    selected: ["get-user", "create-user"],
  });
  p.dom.window.close();
});
test("搜索不改变文件夹操作范围，接口菜单仅导出当前接口", () => {
  const p = panel();
  p.$("scan").click();
  p.reply({ op: "scan", export: treeExport() });
  p.$("search").value = "创建";
  p.$("search").dispatchEvent(new p.window.Event("input"));
  assert.equal(p.$("list").querySelectorAll("[data-endpoint]").length, 1);
  assert.ok(groups(p).every((n) => n.open));
  context(p, findGroup(p, "sample-api").querySelector(":scope > summary"));
  assert.match(p.$("context-label").textContent, /3 个接口/);
  action(p, "yaml");
  assert.deepEqual(p.sent.at(-1).selected, [
    "get-user",
    "create-user",
    "get-tags",
  ]);
  assert.equal(p.sent.at(-1).format, "yaml");
  p.reply({ op: "notice", message: "已导出" });
  context(p, p.$("list").querySelector("[data-endpoint]"));
  action(p, "json");
  assert.deepEqual(p.sent.at(-1), {
    op: "export",
    scope: "project",
    selected: ["create-user"],
    format: "json",
  });
  p.dom.window.close();
});
test("树叶名称和路径均可预览接口，展开全部与空搜索结果可恢复", () => {
  const p = panel();
  p.reply({ op: "scan", export: treeExport() });
  p.$("expand-all").click();
  assert.ok(groups(p).every((n) => n.open));
  const row = p.$("list").querySelector('[data-endpoint="2"]');
  row.querySelector(".tree-path").click();
  assert.equal(JSON.parse(p.$("json").textContent).operationId, "get-tags");
  assert.equal(row.classList.contains("is-active"), true);
  p.$("search").value = "不存在的接口";
  p.$("search").dispatchEvent(new p.window.Event("input"));
  assert.match(p.$("list").textContent, /没有匹配的接口/);
  p.$("search").value = "";
  p.$("search").dispatchEvent(new p.window.Event("input"));
  assert.equal(p.$("list").querySelectorAll("[data-endpoint]").length, 4);
  p.dom.window.close();
});

test("删除所有勾选框、底部批量按钮及定位按钮，双击接口跳转源码", () => {
  const p = panel();
  p.reply({ op: "scan", export: treeExport() });
  assert.equal(
    p.window.document.querySelectorAll('input[type="checkbox"]').length,
    0,
  );
  assert.equal(p.$("upload"), null);
  assert.equal(p.$("export"), null);
  assert.equal(p.$("model").querySelector("button"), null);
  const title = p
    .$("list")
    .querySelector('[data-endpoint="2"] .endpoint-title');
  title.click();
  assert.equal(JSON.parse(p.$("json").textContent).operationId, "get-tags");
  assert.equal(p.sent.filter((v) => v.op === "navigate").length, 0);
  title.dispatchEvent(
    new p.window.MouseEvent("dblclick", { bubbles: true, cancelable: true }),
  );
  assert.deepEqual(p.sent.at(-1), { op: "navigate", operationId: "get-tags" });
  p.dom.window.close();
});
test("菜单键盘操作、Escape、点击外部和滚动均能关闭菜单", () => {
  const p = panel();
  p.reply({ op: "scan", export: treeExport() });
  const summary = findGroup(p, "sample-api").querySelector(":scope > summary");
  summary.dispatchEvent(
    new p.window.KeyboardEvent("keydown", {
      key: "F10",
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    }),
  );
  const menu = p.$("context-menu");
  assert.equal(menu.classList.contains("hidden"), false);
  assert.equal(p.window.document.activeElement.dataset.action, "upload");
  menu.dispatchEvent(
    new p.window.KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
  );
  assert.equal(p.window.document.activeElement.dataset.action, "yaml");
  menu.dispatchEvent(
    new p.window.KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
  assert.equal(menu.classList.contains("hidden"), true);
  assert.equal(p.window.document.activeElement, summary);
  context(p, summary);
  p.$("search").dispatchEvent(
    new p.window.Event("pointerdown", { bubbles: true }),
  );
  assert.equal(menu.classList.contains("hidden"), true);
  context(p, summary);
  p.$("list").dispatchEvent(new p.window.Event("scroll"));
  assert.equal(menu.classList.contains("hidden"), true);
  p.dom.window.close();
});
test("右键根节点可导出全部，重新扫描和忙碌状态不会执行旧菜单", () => {
  const p = panel();
  p.$("scan").click();
  p.reply({ op: "scan", export: treeExport() });
  context(p, findGroup(p, "接口数").querySelector(":scope > summary"));
  action(p, "json");
  assert.equal(p.sent.at(-1).selected.length, 4);
  const count = p.sent.length;
  context(p, findGroup(p, "接口数").querySelector(":scope > summary"));
  action(p, "upload");
  assert.equal(p.sent.length, count);
  p.reply({ op: "notice", message: "已导出" });
  context(p, findGroup(p, "接口数").querySelector(":scope > summary"));
  p.reply({ op: "scan", export: exp });
  assert.equal(p.$("context-menu").classList.contains("hidden"), true);
  action(p, "upload");
  assert.equal(p.sent.length, count);
  p.dom.window.close();
});
test("重新渲染时直接载入保存配置，不依赖初始化消息回复", () => {
  const state = {
    op: "config",
    config: {
      project_id: "12345",
      endpoint_folder: 76,
      title: "测试项目",
      base_url: "http://localhost:9000",
    },
    projectRoot: "/projects/sample-project",
    tokenSet: true,
  };
  const p = panel(state);
  assert.equal(p.$("project_id").value, "12345");
  assert.equal(p.$("endpoint_folder").value, "76");
  assert.equal(p.$("title").value, "测试项目");
  assert.equal(p.sent.length, 0);
  assert.match(p.$("config-project").textContent, /sample-project/);
  p.$("settings-toggle").click();
  assert.deepEqual(p.sent.at(-1), { op: "init" });
  p.reply(state);
  assert.equal(p.$("project_id").value, "12345");
  p.dom.window.close();
});
test("未保存的编辑不在重新打开面板时被覆盖，保存后以宿主确认值为准", () => {
  const state = {
    op: "config",
    config: {
      project_id: "12345",
      title: "旧配置",
      base_url: "http://localhost:9000",
      overwrite: "AUTO_MERGE",
    },
    projectRoot: "/project",
    tokenSet: false,
  };
  const p = panel(state);
  p.$("settings-toggle").click();
  p.reply(state);
  p.$("project_id").value = "67890";
  p.$("project_id").dispatchEvent(new p.window.Event("input"));
  p.$("settings-toggle").click();
  const count = p.sent.length;
  p.$("settings-toggle").click();
  assert.equal(p.sent.length, count);
  assert.equal(p.$("project_id").value, "67890");
  p.$("save-config").click();
  assert.equal(p.sent.at(-1).config.project_id, "67890");
  p.reply({ op: "error", message: "保存失败" });
  assert.equal(p.$("project_id").value, "67890");
  assert.match(p.$("status").textContent, /保存失败/);
  p.$("save-config").click();
  p.reply({
    ...state,
    saved: true,
    message: "配置已保存",
    config: { ...state.config, project_id: "67890" },
  });
  assert.match(p.$("status").textContent, /配置已保存/);
  p.$("settings-toggle").click();
  p.$("settings-toggle").click();
  assert.deepEqual(p.sent.at(-1), { op: "init" });
  p.dom.window.close();
});
test("迟到的配置刷新回复不会覆盖已经开始输入的新值", () => {
  const state = {
    op: "config",
    config: { project_id: "12345", title: "旧配置" },
    projectRoot: "/project",
    tokenSet: false,
  };
  const p = panel(state);
  p.$("settings-toggle").click();
  p.$("project_id").value = "67890";
  p.$("project_id").dispatchEvent(new p.window.Event("input"));
  p.reply(state);
  assert.equal(p.$("project_id").value, "67890");
  assert.equal(p.$("save-config").disabled, false);
  p.dom.window.close();
});
const remoteChoices = [
  { id: "11", name: "同名项目", team_id: "1", team_name: "团队 A" },
  { id: "22", name: "同名项目", team_id: "2", team_name: "团队 B" },
  { id: "33", name: "其他项目", team_id: "2", team_name: "团队 B" },
];
const pickerState = {
  op: "config",
  config: {
    project_id: "11",
    project_name: "同名项目",
    team_name: "团队 A",
    endpoint_folder: 7,
    schema_folder: 8,
    branch: 9,
    module: 10,
    title: "文档",
    base_url: "http://localhost:8080",
    overwrite: "AUTO_MERGE",
  },
  projectRoot: "/project",
  tokenSet: true,
};

const visiblePicker = (p) =>
  !p.$("project-picker").classList.contains("hidden");
const chooseOption = (p, id) =>
  p.$("project-choice").querySelector(`[data-project-id="${id}"]`).click();
const key = (p, target, value) =>
  p.$(target).dispatchEvent(
    new p.window.KeyboardEvent("keydown", {
      key: value,
      bubbles: true,
      cancelable: true,
    }),
  );
test("单一同步项目入口展示已选目标，首次点开自动加载并复用列表", () => {
  const p = panel(pickerState);
  assert.equal(p.$("project-name").textContent, "同名项目");
  assert.equal(p.$("project-meta").textContent, "团队 A · ID 11");
  assert.equal(p.$("project-manual").classList.contains("hidden"), true);
  assert.equal(p.window.document.querySelector("select#project-choice"), null);
  p.$("project-trigger").click();
  assert.deepEqual(p.sent.at(-1), { op: "list-projects" });
  assert.equal(visiblePicker(p), true);
  assert.equal(p.window.document.activeElement, p.$("project-filter"));
  p.reply({ op: "projects", projects: remoteChoices, notices: [] });
  assert.equal(p.$("project_id").value, "11");
  assert.equal(p.$("endpoint_folder").value, "7");
  assert.equal(
    p.$("project-choice").querySelectorAll('[role="option"]').length,
    3,
  );
  assert.equal(
    p
      .$("project-choice")
      .querySelector('[data-project-id="11"]')
      .getAttribute("aria-selected"),
    "true",
  );
  p.$("project-trigger").click();
  assert.equal(visiblePicker(p), false);
  const count = p.sent.length;
  p.$("project-trigger").click();
  assert.equal(p.sent.length, count);
  p.$("load-projects").click();
  assert.equal(p.sent.length, count + 1);
  p.dom.window.close();
});
test("选择后立即收起并聚焦入口，切换项目清理旧目录，保存名称不变", () => {
  const p = panel(pickerState);
  p.$("project-trigger").click();
  p.reply({ op: "projects", projects: remoteChoices, notices: [] });
  chooseOption(p, "22");
  assert.equal(visiblePicker(p), false);
  assert.equal(p.window.document.activeElement, p.$("project-trigger"));
  assert.equal(p.$("project_id").value, "22");
  for (const id of ["endpoint_folder", "schema_folder", "branch", "module"])
    assert.equal(p.$(id).value, "");
  assert.equal(p.$("project-name").textContent, "同名项目");
  assert.equal(p.$("project-meta").textContent, "团队 B · ID 22");
  assert.match(p.$("project-change-status").textContent, /保存后生效/);
  assert.equal(p.sent.filter((v) => v.op === "save-config").length, 0);
  p.$("save-config").click();
  assert.equal(p.sent.at(-1).config.project_name, "同名项目");
  assert.equal(p.sent.at(-1).config.team_name, "团队 B");
  assert.equal(p.sent.at(-1).config.endpoint_folder, null);
  p.dom.window.close();
});
test("搜索加方向键 Enter 可直接选择，Escape 取消且不改原项目", () => {
  const p = panel(pickerState);
  p.$("project-trigger").click();
  p.reply({ op: "projects", projects: remoteChoices, notices: [] });
  p.$("project-filter").value = "同名";
  p.$("project-filter").dispatchEvent(new p.window.Event("input"));
  assert.equal(
    p.$("project-choice").querySelectorAll('[role="option"]').length,
    2,
  );
  key(p, "project-filter", "ArrowDown");
  assert.equal(
    p.$("project-filter").getAttribute("aria-activedescendant"),
    "project-option-22",
  );
  key(p, "project-filter", "Enter");
  assert.equal(p.$("project_id").value, "22");
  assert.equal(visiblePicker(p), false);
  p.$("project-trigger").click();
  assert.equal(p.$("project-filter").value, "");
  p.$("project-filter").value = "33";
  p.$("project-filter").dispatchEvent(new p.window.Event("input"));
  key(p, "project-filter", "Escape");
  assert.equal(p.$("project_id").value, "22");
  assert.equal(visiblePicker(p), false);
  assert.equal(p.$("project-trigger").getAttribute("aria-expanded"), "false");
  p.dom.window.close();
});
test("选择同一项目保留目录，搜索与取消不会标记配置为已编辑", () => {
  const p = panel(pickerState);
  p.$("project-trigger").click();
  p.reply({ op: "projects", projects: remoteChoices, notices: [] });
  p.$("project-filter").value = "22";
  p.$("project-filter").dispatchEvent(new p.window.Event("input"));
  key(p, "project-filter", "Escape");
  p.$("settings-toggle").click();
  assert.equal(p.sent.at(-1).op, "init");
  p.reply(pickerState);
  p.$("project-trigger").click();
  chooseOption(p, "11");
  assert.equal(p.$("endpoint_folder").value, "7");
  assert.equal(p.$("project-change-status").classList.contains("hidden"), true);
  p.dom.window.close();
});
test("加载期间关闭后回复不再弹出，空列表不会反复自动加载", () => {
  const p = panel(pickerState);
  p.$("project-trigger").click();
  const count = p.sent.length;
  p.$("project-trigger").click();
  p.reply({ op: "projects", projects: [], notices: [] });
  assert.equal(visiblePicker(p), false);
  assert.equal(p.$("project_id").value, "11");
  p.$("project-trigger").click();
  assert.equal(p.sent.length, count);
  assert.match(p.$("project-list-status").textContent, /没有可用/);
  assert.equal(p.$("project-choice").children.length, 0);
  p.$("search").dispatchEvent(
    new p.window.Event("pointerdown", { bubbles: true }),
  );
  assert.equal(visiblePicker(p), false);
  p.dom.window.close();
});
test("错误可以刷新或手动输入，手动入口默认隐藏且不会覆盖原项目", () => {
  const p = panel(pickerState);
  p.$("project-trigger").click();
  p.reply({ op: "error", message: "令牌过期" });
  assert.equal(p.$("project_id").value, "11");
  assert.match(p.$("project-list-status").textContent, /令牌过期/);
  assert.equal(p.$("load-projects").disabled, false);
  p.$("project-manual-toggle").click();
  assert.equal(visiblePicker(p), false);
  assert.equal(p.$("project-manual").classList.contains("hidden"), false);
  p.$("project_id").value = "";
  p.$("project-manual-done").click();
  assert.equal(p.$("project-manual").classList.contains("hidden"), false);
  p.$("project_id").value = "555";
  p.$("project_id").dispatchEvent(new p.window.Event("input"));
  p.$("project-manual-done").click();
  assert.equal(p.$("project-manual").classList.contains("hidden"), true);
  assert.equal(p.$("project-name").textContent, "项目 555");
  p.$("save-config").click();
  assert.equal(p.sent.at(-1).config.project_id, "555");
  assert.equal(p.sent.at(-1).config.project_name, "");
  p.dom.window.close();
});
test("项目名称作为文本渲染，无结果搜索清楚提示且不修改配置", () => {
  const p = panel(pickerState);
  p.$("project-trigger").click();
  p.reply({
    op: "projects",
    projects: [
      {
        id: "22",
        name: "<img src=x onerror=alert(1)>",
        team_name: "团队",
        team_id: "2",
      },
    ],
    notices: [],
  });
  assert.equal(p.window.document.querySelectorAll("img").length, 0);
  p.$("project-filter").value = "不存在";
  p.$("project-filter").dispatchEvent(new p.window.Event("input"));
  assert.match(p.$("project-list-status").textContent, /没有匹配/);
  assert.equal(p.$("project_id").value, "11");
  assert.equal(
    p.$("project-filter").hasAttribute("aria-activedescendant"),
    false,
  );
  p.dom.window.close();
});
test("配置刷新尚未结束时点开选择器，会在刷新后自动加载项目", () => {
  const p = panel(pickerState);
  p.$("settings-toggle").click();
  assert.equal(p.sent.at(-1).op, "init");
  p.$("project-trigger").click();
  assert.equal(p.sent.at(-1).op, "init");
  p.reply(pickerState);
  assert.equal(p.sent.at(-1).op, "list-projects");
  assert.equal(visiblePicker(p), true);
  p.dom.window.close();
});
test("长包名使用整行分组且保留完整文字，树深度不会持续挤压名称宽度", () => {
  const p = panel();
  const e = treeExport();
  e.endpoints = e.endpoints.slice(0, 1);
  e.endpoints[0].package =
    "org.dromara.system.controller.verylongpackagename.withdeepbranches";
  e.endpoints[0].controller_name = "很长的控制器名称用于窄侧栏换行展示";
  e.endpoints[0].path =
    "/system/very-long-path/with/many/segments/and-a-long-name";
  e.endpoints[0].operation.summary =
    "这是一个很长的接口名称用于确认不会被省略截断";
  p.reply({ op: "scan", export: e });
  p.$("expand-all").click();
  const group = findGroup(p, e.endpoints[0].package);
  assert.ok(group);
  const summary = group.querySelector(":scope > summary");
  assert.equal(summary.children.length, 1);
  assert.equal(
    summary.querySelector(".tree-label").textContent,
    e.endpoints[0].package,
  );
  assert.equal(
    p.window.getComputedStyle(summary).gridTemplateColumns.replace(/\s/g, ""),
    "minmax(0,1fr)",
  );
  const row = p.$("list").querySelector("[data-endpoint]");
  assert.equal(row.querySelector(".route").textContent, e.endpoints[0].path);
  assert.equal(
    row.querySelector(".endpoint-title").textContent,
    e.endpoints[0].operation.summary,
  );
  assert.equal(
    p.window.getComputedStyle(row.querySelector(".endpoint-title")).whiteSpace,
    "normal",
  );
  assert.equal(
    p.window.getComputedStyle(row.querySelector(".route")).overflowWrap,
    "anywhere",
  );
  assert.equal(p.window.getComputedStyle(p.$("list")).overflowX, "hidden");
  for (const node of p.$("list").querySelectorAll(".api-tree-row"))
    assert.ok(Number(node.style.getPropertyValue("--tree-depth")) <= 5);
  p.dom.window.close();
});
test("窄树布局仍支持接口右键、预览和双击导航", () => {
  const p = panel();
  p.reply({ op: "scan", export: treeExport() });
  p.$("expand-all").click();
  const row = p.$("list").querySelector('[data-endpoint="1"]');
  row.querySelector(".endpoint-title").click();
  assert.equal(JSON.parse(p.$("json").textContent).operationId, "create-user");
  context(p, row);
  action(p, "json");
  assert.deepEqual(p.sent.at(-1).selected, ["create-user"]);
  p.reply({ op: "notice", message: "已导出" });
  row
    .querySelector(".tree-path")
    .dispatchEvent(
      new p.window.MouseEvent("dblclick", { bubbles: true, cancelable: true }),
    );
  assert.deepEqual(p.sent.at(-1), {
    op: "navigate",
    operationId: "create-user",
  });
  p.dom.window.close();
});
