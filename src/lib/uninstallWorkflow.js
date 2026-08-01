const ALLOWED = {
  analyzing: { ANALYSIS_DONE: "overview", FAIL: "error" },
  overview: { RESCAN_START: "analyzing", PREPARE_START: "preparing", FAIL: "error" },
  preparing: { BASELINE_READY: "launching", FAIL: "error" },
  launching: { LAUNCHED: "waiting", FAIL: "error" },
  waiting: { APP_REMOVED: "scanning", UNINSTALL_CANCELLED: "overview", FAIL: "error" },
  scanning: { SCAN_DONE: "review", FAIL: "error" },
  review: { CLEAN_START: "cleaning", FAIL: "error" },
  cleaning: { CLEAN_DONE: "complete", UNDO_DONE: "review", FAIL: "error" },
  complete: { UNDO_START: "cleaning", RESET: "overview" },
  error: { RETRY_ANALYSIS: "analyzing", RETRY_OVERVIEW: "overview", RETRY_WAITING: "waiting" },
};

export function createWorkflow(app = null, footprint = null) {
  return {
    phase: app ? "analyzing" : "idle",
    app,
    footprint,
    result: null,
    error: "",
    recoverPhase: "analyzing",
  };
}

export function transition(state, event, payload = {}) {
  const nextPhase = ALLOWED[state.phase]?.[event];
  if (!nextPhase) {
    throw new Error(`非法状态转换：${state.phase} -> ${event}`);
  }
  return {
    ...state,
    ...payload,
    phase: nextPhase,
    error: event === "FAIL" ? String(payload.error || "操作失败") : "",
  };
}

export function summarizeFootprint(footprint = {}) {
  const artifacts = footprint.artifacts || [];
  const sum = (predicate) =>
    artifacts.reduce((total, artifact) => total + (predicate(artifact) ? Number(artifact.size) || 0 : 0), 0);
  return {
    totalSize: Number(footprint.total_size) || sum((artifact) => artifact.kind === "dir"),
    programSize: sum(
      (artifact) => artifact.kind === "dir" && (artifact.source === "install-dir" || artifact.category === "binary")
    ),
    appDataSize: sum(
      (artifact) =>
        artifact.kind === "dir" &&
        artifact.source !== "install-dir" &&
        artifact.category !== "binary" &&
        artifact.category !== "user-data"
    ),
    userDataSize: sum((artifact) => artifact.kind === "dir" && artifact.category === "user-data"),
    registryCount: artifacts.filter((artifact) => artifact.kind === "reg-key").length,
    serviceCount: artifacts.filter((artifact) => artifact.kind === "service").length,
    startupCount: artifacts.filter((artifact) => artifact.kind === "startup").length,
  };
}

export function defaultSelectedPaths(artifacts = [], appRemoved = false) {
  if (!appRemoved) return new Set();
  return new Set(
    artifacts
      .filter(
        (artifact) =>
          (artifact.kind === "dir" || artifact.kind === "reg-key") &&
          artifact.source?.startsWith("residue") &&
          artifact.confidence === "high" &&
          artifact.category !== "user-data"
      )
      .map((artifact) => artifact.path)
  );
}

export function recommendationLabel(score = {}) {
  if (score.caution) return "谨慎处理";
  const value = score.removal_recommendation || 0;
  if (value >= 50) return "可考虑卸载";
  if (value >= 20) return "建议评估";
  return "暂不建议";
}

export function isBusy(phase) {
  return ["analyzing", "preparing", "launching", "scanning", "cleaning"].includes(phase);
}

export function isCurrentRequest(currentAppId, currentSeq, responseAppId, responseSeq) {
  return currentAppId === responseAppId && currentSeq === responseSeq;
}

export function canOpenArtifact(artifact) {
  return artifact?.kind === "dir" && typeof artifact.path === "string" && artifact.path.trim().length > 0;
}
