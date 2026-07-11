# Win-Top 发版流程规范

适用于每次向 GitHub 发布新版本的完整流程：版本号 → 构建 → 验证 → 提交打标 → GitHub Release。除"本地验证"外均可按命令逐步执行。

---

## 1. 版本号

版本号需在 **三处** 同步修改（保持一致）：

| 文件 | 字段 |
| --- | --- |
| `package.json` | `version` |
| `src-tauri/tauri.conf.json` | `package.version` |
| `src-tauri/Cargo.toml` | `[package] version` |

版本策略：`0.x.y` 阶段功能批次与修复均递增补丁号（如 0.1.2 → 0.1.3）；出现不兼容的配置/数据变更时递增次版本号。

## 2. 构建前检查

- **关闭所有正在运行的 Win-Top 实例**（包括提权实例），否则 cargo 链接阶段替换 exe 会报"拒绝访问"。
- 工作区应只包含本次发版相关的变更；`git status` 确认无意外改动。

## 3. 构建

```bash
npm run tauri build
```

该命令自动完成：`vite build` 前端产物 → cargo release 编译（自动附带 `custom-protocol` feature）→ 生成 MSI 与 NSIS 安装包。

便携版由 release 主程序复制重命名生成：

```powershell
New-Item -ItemType Directory -Force src-tauri\target\release\bundle\portable | Out-Null
Copy-Item src-tauri\target\release\Win-Top.exe "src-tauri\target\release\bundle\portable\Win-Top_{版本}_portable_x64.exe"
```

## 4. 产物与命名规范

三个发布产物，命名必须与下表一致（`{版本}` 形如 `0.1.3`）：

| 产物 | 路径 | Release 内说明 |
| --- | --- | --- |
| `Win-Top_{版本}_portable_x64.exe` | `src-tauri/target/release/bundle/portable/` | 免安装版，双击即用（推荐快速体验） |
| `Win-Top_{版本}_x64-setup.exe` | `src-tauri/target/release/bundle/nsis/` | NSIS 安装程序 |
| `Win-Top_{版本}_x64_en-US.msi` | `src-tauri/target/release/bundle/msi/` | MSI 安装包 |

## 5. 本地验证（发布前必做）

用安装包或便携版实际安装/运行一遍，检查：

- [ ] 应用正常启动，标题栏 / 关于页版本号正确
- [ ] 八个视图（概览 / 进程 / 实时事件 / 网络端口 / 磁盘 / 优化加速 / 系统工具 / AI 助手）均能打开且数据正常
- [ ] 「以管理员重启」后：实时事件有事件流、磁盘温度可显示
- [ ] AI 助手多轮对话正常（本次改动涉及的模块重点回归）
- [ ] 本次版本的新增 / 修复点逐项复验

## 6. 提交与打标

提交遵循 Conventional Commits（`feat / fix / perf / refactor / docs / chore` + 模块 scope），要求：

- 功能、修复、文档等变更**先各自独立提交**；
- 版本号变更**单独提交**，信息固定为：`chore(release): 版本号升至 {版本}`；
- 提交信息中性描述变更后的状态与内容，不携带任何工具署名。

```bash
git push origin master
git tag v{版本}
git push origin v{版本}
```

## 7. GitHub Release

在 Releases 页 **Draft a new release**（不要编辑历史 Release）：

1. **Choose a tag** → 选择已推送的 `v{版本}`；
2. **标题**：`Win-Top v{版本} {本次核心亮点}`（如 `Win-Top v0.1.3 AI 系统诊断助手`）；
3. **正文**：按下方模板撰写；
4. 上传第 4 节的三个产物；
5. 勾选 **Set as the latest release** → **Publish release**。

或使用 gh CLI 一步完成：

```bash
gh release create v{版本} --latest --title "Win-Top v{版本} {亮点}" --notes-file notes.md ^
  "src-tauri\target\release\bundle\portable\Win-Top_{版本}_portable_x64.exe" ^
  "src-tauri\target\release\bundle\nsis\Win-Top_{版本}_x64-setup.exe" ^
  "src-tauri\target\release\bundle\msi\Win-Top_{版本}_x64_en-US.msi"
```

### Release 说明模板

```markdown
## 更新内容

### 新增
- **{功能名}**：{一句话说明；子项用缩进列表}

### 优化
- {用户可感知的改进}

### 修复
- 修复{问题现象}的问题。

## 下载说明
- `Win-Top_{版本}_portable_x64.exe`：免安装版，双击即用（推荐快速体验）
- `Win-Top_{版本}_x64-setup.exe`：NSIS 安装程序
- `Win-Top_{版本}_x64_en-US.msi`：MSI 安装包

> 系统要求 Windows 10 1809+ / Windows 11（x64），依赖 WebView2 运行时（Win11 自带）。实时事件与磁盘温度需管理员权限，可在应用内一键「以管理员重启」。安装包未做代码签名，SmartScreen 提示「未知发布者」时选择「仍要运行」即可。

**完整变更**：https://github.com/nickshui/Win-Top/compare/v{上一版本}...v{版本}
```

### 说明撰写规则

- 变更来源：`git log v{上一版本}..v{版本} --oneline`，按 新增（feat）/ 优化（perf、refactor 中用户可感知部分）/ 修复（fix）归类；
- 只写**用户可感知**的变化，内部重构 / 文档 / 构建类提交不列入（除非直接影响体验）；
- 上一版本已发布过的内容不得重复出现；
- 修复条目写用户视角的"问题现象"，不写内部实现细节；
- 「新增」中的重点功能加粗功能名，可用缩进子项展开说明。

## 8. 发布后检查

- [ ] Release 页三个产物可正常下载；
- [ ] README 顶部 release 徽章显示新版本号；
- [ ] 安装包下载后可安装运行（SmartScreen 提示属预期）。
