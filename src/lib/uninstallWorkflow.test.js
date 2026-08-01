import test from "node:test";
import assert from "node:assert/strict";
import {
  canOpenArtifact,
  createWorkflow,
  transition,
  defaultSelectedPaths,
  summarizeFootprint,
  recommendationLabel,
  isCurrentRequest,
} from "./uninstallWorkflow.js";

test("选择应用后先分析，完成后才能进入只读概况", () => {
  let state = createWorkflow({ entry: { id: "app-1" }, score: {} });
  assert.equal(state.phase, "analyzing");
  assert.throws(() => transition(state, "PREPARE_START"), /非法状态转换/);
  state = transition(state, "ANALYSIS_DONE", { footprint: { artifacts: [] } });
  assert.equal(state.phase, "overview");
  assert.throws(() => transition(state, "CLEAN_START"), /非法状态转换/);
});

test("只有检测到应用消失并复扫成功后才能复核残留", () => {
  let state = createWorkflow({ entry: { id: "app-1" }, score: {} });
  state = transition(state, "ANALYSIS_DONE", { footprint: { app_id: "app-1", artifacts: [] } });
  state = transition(state, "PREPARE_START");
  state = transition(state, "BASELINE_READY", { footprint: { app_id: "app-1" } });
  state = transition(state, "LAUNCHED");
  state = transition(state, "APP_REMOVED");
  assert.equal(state.phase, "scanning");
  state = transition(state, "SCAN_DONE", { footprint: { artifacts: [] } });
  assert.equal(state.phase, "review");
});

test("应用概况按真实产物分类汇总体积和系统项", () => {
  const summary = summarizeFootprint({
    total_size: 960,
    artifacts: [
      { kind: "dir", source: "install-dir", category: "binary", size: 500 },
      { kind: "dir", source: "residue-fs", category: "binary", size: 50 },
      { kind: "dir", source: "residue-fs", category: "cache", size: 120 },
      { kind: "dir", source: "residue-fs", category: "config", size: 80 },
      { kind: "dir", source: "residue-fs", category: "app-runtime", size: 60 },
      { kind: "dir", source: "residue-fs", category: "user-data", size: 150 },
      { kind: "reg-key", source: "residue-reg", category: "config", size: 0 },
      { kind: "service", source: "service", category: "app-runtime", size: 0 },
      { kind: "startup", source: "startup", category: "app-runtime", size: 0 },
    ],
  });
  assert.deepEqual(summary, {
    totalSize: 960,
    programSize: 550,
    appDataSize: 260,
    userDataSize: 150,
    registryCount: 1,
    serviceCount: 1,
    startupCount: 1,
  });
});

test("卸载后只默认选择高置信的非用户数据残留", () => {
  const artifacts = [
    { path: "C:\\Safe", kind: "dir", source: "residue-fs", confidence: "high", category: "cache" },
    { path: "C:\\Review", kind: "dir", source: "residue-fs", confidence: "medium", category: "config" },
    { path: "C:\\Install", kind: "dir", source: "install-dir", confidence: "high", category: "binary" },
    { path: "C:\\User", kind: "dir", source: "residue-fs", confidence: "high", category: "user-data" },
  ];
  assert.deepEqual([...defaultSelectedPaths(artifacts, true)], ["C:\\Safe"]);
  assert.equal(defaultSelectedPaths(artifacts, false).size, 0);
});

test("建议标签使用可解释等级且谨慎标记优先", () => {
  assert.equal(recommendationLabel({ removal_recommendation: 80, caution: true }), "谨慎处理");
  assert.equal(recommendationLabel({ removal_recommendation: 60 }), "可考虑卸载");
  assert.equal(recommendationLabel({ removal_recommendation: 25 }), "建议评估");
  assert.equal(recommendationLabel({ removal_recommendation: 10 }), "暂不建议");
});

test("过期请求不能覆盖当前应用", () => {
  assert.equal(isCurrentRequest("app-2", 4, "app-1", 3), false);
  assert.equal(isCurrentRequest("app-2", 4, "app-2", 4), true);
});

test("分析失败可以重试，卸载能力失败可以返回概况", () => {
  let state = createWorkflow({ entry: { id: "app-1" }, score: {} });
  state = transition(state, "FAIL", { error: "扫描失败", recoverPhase: "analyzing" });
  assert.equal(state.phase, "error");
  state = transition(state, "RETRY_ANALYSIS");
  assert.equal(state.phase, "analyzing");
  state = transition(state, "ANALYSIS_DONE", { footprint: { artifacts: [] } });
  state = transition(state, "FAIL", { error: "没有卸载命令", recoverPhase: "overview" });
  state = transition(state, "RETRY_OVERVIEW");
  assert.equal(state.phase, "overview");
});

test("取消厂商卸载后可以退出等待并回到应用概况", () => {
  let state = createWorkflow({ entry: { id: "app-1" }, score: {} });
  state = transition(state, "ANALYSIS_DONE", { footprint: { artifacts: [] } });
  state = transition(state, "PREPARE_START");
  state = transition(state, "BASELINE_READY");
  state = transition(state, "LAUNCHED");
  assert.equal(state.phase, "waiting");

  state = transition(state, "UNINSTALL_CANCELLED");
  assert.equal(state.phase, "overview");
  assert.equal(state.error, "");
});

test("只有真实文件系统目录提供打开操作", () => {
  assert.equal(canOpenArtifact({ kind: "dir", path: "C:\\ProgramData\\Test App" }), true);
  assert.equal(canOpenArtifact({ kind: "reg-key", path: "HKCU\\Software\\Test App" }), false);
  assert.equal(canOpenArtifact({ kind: "service", path: "TestService" }), false);
  assert.equal(canOpenArtifact({ kind: "dir", path: "" }), false);
});
