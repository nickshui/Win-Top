# Win-Top 构建环境说明

## 1) 目标平台

Win-Top 的桌面运行目标是 **Windows**。  
在非 Windows 环境（如 Linux CI）中，项目当前仅保证可以进行基础 `cargo check` 级别的代码检查，不保证可运行桌面窗口。

---

## 2) glib-2.0 / GTK 相关说明

如果你在 Linux 上构建启用了 Tauri Linux 运行时的目标，会遇到：

- `glib-2.0` 缺失
- `webkit2gtk` / `gtk3` 缺失

典型报错：

- `The system library glib-2.0 required by crate glib-sys was not found`

### Ubuntu/Debian 依赖示例

```bash
sudo apt-get update
sudo apt-get install -y \
  pkg-config \
  libglib2.0-dev \
  libgtk-3-dev \
  libwebkit2gtk-4.1-dev
```

> 注意：若你的网络环境受代理限制（如 403），请先修复 apt 源/代理，否则安装会失败。

---

## 3) 当前仓库策略（已实现）

- `tauri` 与 `tauri-build` 已限定在 `cfg(windows)` 目标下依赖。
- 非 Windows 下使用降级 `main`，用于静态检查流程。

这能避免在 Linux 代码检查路径上被 `glib-2.0` 阻塞。

---

## 4) 推荐检查命令

### Windows 开发机

```bash
cd src-tauri
cargo check
```

### Linux/CI（无桌面依赖）

```bash
cd src-tauri
cargo check --offline
```

