# AuraQuota

极简中文、玻璃拟态（Glassmorphism / Windows 11 Acrylic）的 Windows 桌面常驻 AI 多账号额度悬浮监控工具，使用 **Tauri 2 + Rust + React + TypeScript + Tailwind CSS** 构建。

## 核心特性

- **6 平台多账号聚合监控（覆盖免费与付费订阅）**：
  - **反重力 (Antigravity)**：AI Pro / Ultra / Free，区分 Gemini 模型池 (5h) 与 Claude / GPT 池 (周)。
  - **Gemini App**：AI Pro / Ultra / Free，独立隔离 WebView2 多账号登录，实时同步 5 小时当前使用与 7 天周限额。
  - **Codex**：Plus / Pro / Free，支持系统默认浏览器一键 OAuth PKCE 授权与本机 CLI 自动读取，自动续期。
  - **Claude**：Pro / Max / Free，支持系统默认浏览器 OAuth 授权、本机 Claude Code 读取与独立窗口登录，自动续期。
  - **Command Code**：GOAT / Go / Pro / Max，监控 5h / 7d / 月度配额并单独展示可结转充值余额（Purchased Credits）。
  - **OpenCode Go**：Go / Go Plus / Go Teams / Free，监控滚动周期与 Console / Zen 余额。
- **三态无边框毛玻璃窗口**：
  - 右上角展开面板（`396×576`） / 悬浮圆球（`72×72`，可拖拽吸附屏幕边缘） / 右侧贴边标签（`36×110`）。
  - 深空（Dark）与浅瓷（Light）双主题，支持隐私模式打码邮箱、系统托盘控制、开机自启与低额度（≤20%）系统提醒。
- **本地安全存储与自动更新**：
  - 所有凭据仅通过 Windows **DPAPI (`CryptProtectData`)** 加密落盘，绝不明文存储。
  - 内置 GitHub Releases 自动检查更新与静默升级支持。

## 本地开发与构建

```bash
pnpm install
pnpm tauri dev
```

打包 Windows NSIS 安装包：

```bash
pnpm tauri build --bundles nsis
```

运行 Rust 单元测试：

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```
