// 版本号的单一来源：package.json 的 version。
//
// 为什么需要这个脚本：
// * tauri.conf.json 的 package.version 指向 "../package.json"（Tauri v1 官方支持的 path 形式），
//   打包版本 / 运行时 getVersion() 都从 package.json 读取；
// * 但 tauri-build 是否重新解析版本取决于 tauri.conf.json 的 mtime，只改 package.json
//   不会触发 Cargo 重新构建（实测会沿用旧版本号）——所以构建前触摸一次配置；
// * Cargo.toml 的 [package].version 不再是应用版本来源，这里保持同步只为避免误解与
//   二进制元数据不一致。
//
// 发布流程：只改 package.json 的 version，然后正常 npm run tauri build（会自动执行本脚本）。
import { readFileSync, writeFileSync, utimesSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pkgPath = join(root, "package.json");
const cargoPath = join(root, "src-tauri", "Cargo.toml");
const confPath = join(root, "src-tauri", "tauri.conf.json");

const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
const version = String(pkg.version || "").trim();
if (!/^\d+\.\d+\.\d+([+-][0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(
    `[sync-version] package.json 的 version 不是合法 semver：${JSON.stringify(version)}`,
  );
  process.exit(1);
}

const conf = JSON.parse(readFileSync(confPath, "utf8"));
if (conf?.package?.version !== "../package.json") {
  console.error(
    '[sync-version] tauri.conf.json 的 package.version 应为 "../package.json"（单一版本来源），' +
      `当前为 ${JSON.stringify(conf?.package?.version)}`,
  );
  process.exit(1);
}

const cargo = readFileSync(cargoPath, "utf8");
const cargoVersionRe = /(\[package\][\s\S]*?\nversion\s*=\s*)"[^"]*"/;
if (!cargoVersionRe.test(cargo)) {
  console.error("[sync-version] 未能在 src-tauri/Cargo.toml 中找到 [package].version");
  process.exit(1);
}
const cargoNext = cargo.replace(cargoVersionRe, `$1"${version}"`);
if (cargoNext !== cargo) writeFileSync(cargoPath, cargoNext);

// 触摸配置：让 cargo 重新运行 tauri-build，使 package.json 的新版本进入构建产物。
const now = new Date();
utimesSync(confPath, now, now);

console.log(
  `[sync-version] v${version} → package.json（源）/ Cargo.toml / tauri.conf.json`,
);
