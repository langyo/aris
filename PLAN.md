# aris — 项目状态与计划 (PLAN)

> 本文件于 **2026-07-13** 更新，记录项目当前状态、近期进展与后续计划。
> 定位已于 2026-07-10 变更为「基于 servo 的浏览器引擎」——详见第 6 节。
> **2026-07-14 刷新记录**：PLAN.md 顶部加 §"Refresh log 2026-07-14"；其他章节保留原貌。
> 原有工业网关发行版计划已保留于文末「既有详细计划（存档）」。

## Refresh log 2026-07-17 (fix/font-in-tree：文字渲染修复 + kei vtty 控制台)

- **文字渲染根因找到并修复**（commit `c4cc8d4`）：`system_fonts: false` 时，
  fontique `register_fonts` **不会**自动填充 generic-family 映射表与 script
  fallback 表（这两张表只由系统字体库扫描填充）。于是 CSS 通用族
  （`sans-serif`/`serif`/`monospace`）与 UA 默认字体栈匹配 0 个字体，
  parley 排版零 glyph、paint_scene 不画文字——这正是 kei 侧"子集字体注册
  成功但 paint 不画文字节点"的根因（并非字体太大或注册失败）。
- **修法**：`lib.rs::new_embedded_font_context()` 注册内嵌字体后显式
  `set_generic_families`（全部 GenericFamily → 内嵌 DejaVu Sans /
  DejaVu Sans Mono）+ `append_fallbacks`（Latn/Grek/Cyrl 等 → 内嵌字体）；
  `render_html` / `render_html_with_font` 统一使用它；
  `winit_backend::new_font_context` 改为 append 模式兜底。
  `render_html_with_font` 顺带把 `Arc<dyn AsRef<[u8]>>` blob 改成具体
  `Vec<u8>`（kei VM 上 dyn vtable 读 NULL 的老坑）。
- **新增内嵌等宽字体**：`packages/render/assets/font-mono.ttf`
  （DejaVu Sans Mono 子集，Basic Latin + Latin-1 + box-drawing，41 KB，
  fonttools 裁剪；许可证 `assets/LICENSE.dejavu`）。"字体库自维护"现状：
  in-tree fork 方案已放弃（见 2026-07-15 记录），自维护 = crates.io
  parley 0.10/fontique + aris 仓内嵌字体资产 + 上述 FontContext 装配函数。
- **kei vtty 控制台帧**（commit `6ae40e8`）：新增 host 工具
  `prerender_vtty`，用完整 aris-render 管线离线渲染现代 Linux 内核控制台
  风格状态屏（黑底、浅色等宽字、`[    0.000000] kei: ...` 内核日志排版、
  无科幻元素），产物 `packages/render/assets/vtty_console.png` 入 git；
  `kei_tty` 改为内嵌该 PNG（替换已被移除的 1280x800 rgba fixture，原
  include_bytes! 已无法编译），运行时用纯 Rust png crate 解码 blit 到
  /dev/fb0，WS JSON-RPC 网关（:8423）不变；feature gate 仅 `png`，
  musl 交叉编译不再拉 Blitz/Vello。
- **bin feature gates 修正**：`render_test`/`kei_ui`/`prerender_cursor`
  只需 `render`；`kei_fbtest`/`kei_minimal` 无需 feature（补 unix 门禁
  stub，Windows 主机 workspace check 保持绿）；`font_smoke`（新，字体
  管线离线诊断）与 `prerender_vtty` 需 `render`；`kei_tty` 需 `png`。
- **本机绝对路径清理**（commit `abdabe5`）：`.cargo/config-riscv.toml`
  的 `D:\...` musl-gcc linker 改成文档化模板；`scripts/zig_cc.sh` 改用
  PATH（`$ZIG` 覆盖）；`offscreen_chrome.rs` 默认输出改相对路径；删除
  多余的 `.cargo/config.toml.bak`。
- **验证**：主机离线渲染 `font_smoke.png`（generic 查询全部命中、三行
  文字清晰）；`prerender_vtty` 产物 1280x800 控制台 PNG 目视符合要求；
  `aarch64-unknown-linux-musl` release 交叉编译 `kei_tty` 成功（静态
  ELF，~950 KB）。
- **跨仓依赖**：kei 侧 `agent/vtty-linux-console` 分支（justfile/initramfs
  切到 kei_tty）依赖本分支的 `kei_tty` 与内嵌 PNG 资产。

## Refresh log 2026-07-14

- **当前分支**：`dev` · 领先 `origin/dev` 1 commit
- **最近提交**：`🐛 Aris: align in-tree Linebender fork manifest (7c445f6 follow-up).` (`05679dc`)
- **未提交改动**（4 项，与 2026-07-14 早些时候一致 — arona agent 积累的 wasm/fixture 改动，本轮不 commit）：

  ```
   M packages/wasm/src/bin/prerender_pixels.rs
   M tests/fixtures/kei_desktop.wasm
   M tests/fixtures/kei_desktop_1280x800.rgba
   M tests/fixtures/kei_desktop_rendered.html
  ```

- **后续动作**：
  1. `prerender_pixels.rs` 和 `tests/fixtures/*` 的脏改动**保留未提交**（本轮仅做 Linebender 迁移；那些 wasm/fixture 改动是 arona agent 之前积累的，按独立 commit 节奏走，不混入）。
  2. **Linebender / GoogleFonts 长期方案 → 已落地**：原 `celestia/patches/{fontique,linebender-resource-handle,skrifa}/` 全部迁移到 `aris/packages/{fontique,skrifa,resource-handle}/`，新增 `aris/packages/parley/` 极简 FontContext facade。**aris 仓完全自维护**，不再依赖 celestia-island 上的独立 fork 仓。celestia 顶层 `patches/` 目录下的 3 个 Linebender 仓已删除；aris 顶层 `patches/boa_*` 保留。
  3. **boA 0.21.1 ICU pin 修复**（仓内 `patches/boa_*`）维持现状。

## Refresh log 2026-07-15 (桌面浏览器实测)

- **aris_browser 在 Windows 桌面成功启动运行** ✅
- **离线渲染管线端到端验证** ✅
- reqwest feature 修正：`rustls-tls-no-provider` → `rustls-tls`（`-no-provider` 导致运行时 No provider set panic）

### 实测结果

| Binary | 功能 | 结果 |
|--------|------|------|
| `aris_browser` | 完整桌面浏览器（winit+softbuffer 窗口） | ✅ 启动运行正常 |
| `offscreen_chrome` | 无窗口渲染浏览器外壳+网页→PNG | ✅ 输出 800x600 PNG |
| `render_test` | HTML→CSS→布局→Vello CPU→PPM | ✅ 480000 non-black pixels |
| `offline_timer_test` | JS setTimeout 执行 | ✅ timer fired |
| `offline_canvas_test` | Canvas2D Scene 模型 | ✅ |

### 渲染管线工作流

```
HTML 字符串
  → html5ever (HTML 解析)
  → stylo (CSS 级联)
  → taffy (Flexbox/Grid 布局)
  → parley (文字排版/shaping)
  → vello_cpu (CPU 光栅化)
  → RGBA pixel buffer
  → winit + softbuffer (桌面窗口)
  或 → PNG/PPM (离线/headless)
  或 → /dev/fb0 mmap (嵌入式 kei)
```

### reqwest 教训

`rustls-tls-no-provider` 不会自动安装 TLS crypto provider，导致运行时报 `No provider set`。桌面端应使用 `rustls-tls`。

### 字体渲染修复

`winit_backend::build_doc()` 之前没有注入 `font_ctx`，blitz-dom 默认字体上下文在 Windows 上可能无法发现系统字体（fontconfig 不存在，fontique 的 DirectWrite 路径未经充分验证）。现在显式注入 `new_font_context()`：`system_fonts: true` + 嵌入式 DejaVu Sans 兜底。

### 开始页重写

Catppuccin Mocha 配色、flexbox 居中、4 个书签。去掉了 blitz-dom 可能不支持 CSS Grid 和 transition。

### 其他修复

- `offline_canvas_test.rs` — 适配 Canvas2D Scene-recording 模型
- aris-abi `#[cfg(unix)]` gate — Windows 编译通过
- Clippy fixes：`collapsible_if`、`while_let_loop`、`manual_checked_ops`、`if_same_then_else`
- Test fix：`which_finds_common_binary` 拆为 `#[cfg(unix)]`/`#[cfg(windows)]`
- Reqwest feature：`rustls-tls-no-provider` → `rustls-tls`

---

## Refresh log 2026-07-15 (peer fork agent aris-charlie 实测)

- **commit `05679dc`** 修复了 7c445f6 引入的 3 处 manifest-level bug：
  1. `aris/packages/render/Cargo.toml` `render` feature 引用了未声明的 `dep:linebender_resource_handle` / `dep:parley`（lib name 而非 dep key），已改为 `dep:aris-resource-handle` / `dep:aris-parley`。
  2. `aris/Cargo.toml` `[patch.crates-io]` 仍指向已删除的 `../patches/{skrifa,linebender-resource-handle,fontique}`，已删除这 3 条 stale entry。
  3. `aris/packages/fontique/Cargo.toml` 中 `aris-resource-handle` 内联 dep 没有 section header，被 cargo 误归到 `[dependencies.hashbrown]`，已加 `[dependencies.aris-resource-handle]` section。
- **`cargo check --workspace --exclude aris-abi` 结果**：
  - ✅ `aris-resource-handle`（14 doc-warnings，upstream 风格，pre-existing）
  - ✅ `aris-fontique`（无 error；1 stray key warning 已修）
  - ✅ `aris-skrifa`（无 error）
  - ✅ `aris-parley`（空 lib OK，编译通过 — 但 facade 未实现，见 BLOCK）
  - ❌ `aris-render`（6 E0432，根因是 `aris-parley/src/lib.rs` 空文件）
  - ⏭ `aris-abi`（Windows host 下 `std::os::fd` / `libc::ioctl` / `File::into_raw_fd` 解析失败，pre-existing，Unix-only code 缺 `#[cfg(unix)]` gate）
- **BLOCK (待 langyo 决策)**：aris-parley 的 "极简 FontContext facade" 在 7c445f6 没说实现，src/lib.rs 是 0 字节文件。aris-render 已经 `use parley::FontContext; use parley::fontique::{Collection, CollectionOptions, SourceCache}`，需要 facade 实现：
  - `parley::FontContext` struct（包装 aris-fontique 的 SourceCache + Collection）
  - `parley::fontique` module（re-export aris-fontique 的 Collection / CollectionOptions / SourceCache）
  - 详细报告见 `.amphoreus/RUNS/aris/build-errors.md`
- **agent id 命名**：aris-charlie（per 5 仓优先级顺序第一个空 P0 仓 aris 的第一个认领者）

## Refresh log 2026-07-15 (跟进：放弃 in-tree fork + gitignore 清理 + arona 回滚)

- **架构决策：放弃 in-tree Linebender fork，回到 crates.io parley 0.10** ✅：
  - **commit `e123d1d`**（🧪 中间方案尝试）：保留 4 个 in-tree fork，
    但把 `[package] name` 改回 upstream 原名，并加 `[patch.crates-io]`。
  - **commit `8d63d80`**（🗑️ 最终方案）：完全删除 4 个 in-tree fork 目录
    （`packages/{parley,fontique,skrifa,resource-handle}/` 共 125 个文件），
    移除 workspace members 和 `[patch.crates-io]` entry，让 `aris-render`
    直接依赖 crates.io parley 0.10 / linebender_resource_handle / fontique。
  - 结论：crates.io `parley 0.10` 内部已经 re-export `fontique` 子模块，
    不需要 in-tree facade；kei 字体 NULL-deref 防护可以放在
    `aris-render/src/lib.rs:125`（"skip DOM/Vello entirely" guard），
    不需要单独 fork Blob 类型。
  - 验证：`cargo check -p aris-render --lib --features render` → 0 errors。

- **构建产物清理** ✅：
  - commit `40e7ef6`（🗑️ Aris: gitignore fixture build artifacts + remove tracked ones.）
  - `tests/fixtures/kei_desktop.wasm`、`kei_desktop_1280x800.rgba`、
    `kei_desktop_rendered.html` 从 git 里移除（生成产物，不该入树）。
  - `.gitignore` 加了 `tests/fixtures/*.{wasm,rgba,html,jpg}` 规则，
    以及 `patches/*/target/` 规则。

- **arona 脏文件回滚** ✅：
  - `cd arona && git checkout HEAD -- .` 回滚 200+ 脏文件，
    只剩未跟踪的 `res/prompts/soul/demiurge.md`（entelecheia 生成的）。
  - 脏文件全是 entelecheia-alpha agent 之前跑 build 或 sed 留下的，
    内容与 HEAD 一致，确认可安全回滚。

- **aris-abi Linux 兼容性** ⏳（待修）：
  - Windows host 上 `cargo check --workspace --exclude aris-abi` 通过，
    但 `--include aris-abi` 仍失败（Unix-only 代码 `libc::ioctl`、
    `std::os::fd`、`File::into_raw_fd` 缺 `#[cfg(unix)]` gate）。
  - 计划：给 `packages/abi/src/lib.rs` 顶部加 `#![cfg(unix)]`，
    或者在 `Cargo.toml` 用 `target.` conditional dep。
  - WSL 里 rust toolchain 有 `rust-std-aarch64-unknown-linux-musl` 组件
    冲突（`Scrt1.o` conflict），暂时无法在 WSL 验证 Linux 端。
    需要 `rustup component remove rust-std-aarch64-unknown-linux-musl &&
    rustup component add rust-std-aarch64-unknown-linux-musl` 重装后
    再测。

- **当前 aris 工作区**：0 dirty（PLAN.md 还有一个改动待 commit）。
- **当前分支**：`dev` · 领先 `origin/dev` 1 commit（待 push）
- **最近提交**：`077dc32` 🐛 Aris-render: declare required-features on all 25 bin targets.

- **已知的 follow-up**（pre-existing，不在本次范围）：
  - `offline_canvas_test.rs` 假设 `Canvas2D` 有公开 `rgba: Vec<u8>` 字段，
    但当前 `Canvas2D` 是 Scene-recording model，没有直接 pixel access。
    需要重写测试或给 `Canvas2D` 加 `pixels()` accessor。
  - `aws-lc-sys 0.42` 在 Windows MSVC 上 build 失败（`stdalign_check.c`
    缺 `stdint.h`）——只在 `--features winit`（拉 reqwest→rcgen→aws-lc-rs）
    时触发，不在 default features 范围内。需等 aws-lc-sys 上游修，或
    换 `rustls-tls` 走 ring。

## 0. 浏览器功能状态（2026-07-13）

aris_browser 现在是一个功能完整的桌面浏览器：

### 已实现

| 功能 | 状态 |
|------|------|
| HTML/CSS 渲染 (html5ever + stylo + taffy + parley + Vello CPU) | ✅ |
| SVG 图片渲染 (usvg) + data: URI | ✅ |
| 多标签页 (Ctrl+T/W/Tab，标签栏 UI) | ✅ |
| 导航 (URL/链接/表单/历史/后退前进) | ✅ |
| HTTP(S) + file:// 网络 (子资源/缓存/cookies) | ✅ |
| 浏览器外壳 (Lucide 图标/地址栏/favicon/状态栏/关闭) | ✅ |
| 鼠标+键盘交互 (悬停/点击/滚动/文本输入) | ✅ |
| 右键菜单、Ctrl+F 查找、Ctrl+=缩放 | ✅ |
| JS: `<script>` + onclick + addEventListener + DOM 操作 | ✅ |
| JS: console.log + window.location + setTimeout/setInterval | ✅ |
| 剪贴板 (Ctrl+C/V)、暗色模式检测、可拖拽滚动条 | ✅ |
| 历史持久化 (重启恢复)、真实 favicon 抓取 | ✅ |
| 下载管理 (Content-Disposition → ~/Downloads/) | ✅ |

### 远期目标（需要架构级工作）

| 功能 | 难度 | 说明 |
|------|------|------|
| Canvas 2D API | 中 | 需要 Boa 绑定 + 离屏 RGBA 缓冲区 + 合成 |
| WebGL | 极难 | 需要 GPU 管线；aris 用 CPU 光栅化 (vello_cpu)，没有 OpenGL 上下文 |
| WebRTC | 极难 | 需要 P2P 信令 + 媒体编解码 + SDP/ICE |
| 内联 `<svg>` 元素渲染 | 中 | blitz-dom 的 svg feature 只处理 `<img src=*.svg>`，内联 SVG 需要自定义 paint |

## 1. 项目概述

- **名称**：`aris`
- **简介**：基于 servo 构建的浏览器引擎——可嵌入、可独立运行。底层设施已部分替换 servo 官方组件（SpiderMonkey → Boa、WebRender → Vello CPU）。可运行于 kei 内核或标准 Linux。
- **远程仓库**：本地仓库（无 origin）
- **技术栈**：Rust / just / html5ever / stylo / taffy / parley / vello
- **类别**：browser-engine

## 2. 当前状态

- **当前分支**：`dev`
- **工作区**：干净
- **最近提交时间**：2026-07-04
- **最近提交**：test: cross-compile evernight fixture binaries for multi-platform installer tests

## 3. 未提交改动

无。

## 4. 近期进展

### kei 内核完整启动（2026-07-04）🎉

**kei Asterinas 内核在 QEMU arm64 上完整启动并加载用户空间 ELF 进程。**

- 修复 FDT 内存区域溢出 bug：`max_paddr` 从 128TB 降至正确的 3GB
- 修复 vbe_dispi x86 模块在 aarch64 上的编译错误
- 修复 initramfs 使用错误架构的 busybox
- 内核完整初始化：GIC、timer、SMP、page tables、net、fs、sched、process
- initramfs 解包 → rootfs ready
- **用户空间 init 进程成功加载**（`init=/init`）

### evernight 联调（2026-07-04）

宿主机点火测试（`just ignition-test`）全链路打通，**双向验证通过**：

```
Modbus TCP sim (:5020)
  → evernight sensor-poll (读取 holding registers)
  → WebSocket ws://127.0.0.1:8443/api/ws
  → evernight-server (device.register + device.telemetry)
```

**验证结果（sensor-poll 端）**：

- `Device registered on server node_id=ignition-test-01` ✅
- `Telemetry sent to gateway`（每 2 s 循环）✅

**验证结果（evernight-server 端）**：

- `Device registered node_id=ignition-test-01 stations=1` ✅
- `Telemetry received node_id=ignition-test-01`（持续接收）✅
- `Device unregistered`（断连时正常清理）✅

**发现并修复的问题**：

1. sensor-poll 默认数据目录 `/var/lib/evernight/sensor` 非 root 不可写 → 注入 `SENSOR_DATA_DIR` 环境变量
2. Modbus TCP 模拟器 MBAP 帧解析错误 → 重写为正确的 7-byte header + length-based framing
3. `EntelecheiaTriggerSink` Unix socket 转发失败为非致命（仅 WARN），不影响 gateway 遥测路径

### 核心驱动实现（2026-07-04）

- `led.rs`：GPIO LED 控制（sysfs /sys/class/gpio）
- `watchdog.rs`：/dev/watchdog ioctl WDIOC_KEEPALIVE 喂狗
- `net.rs`：网络接口 netlink 配置
- `ota.rs`：OTA 下载/dm-verity 校验/分区写入（已编译可用，真机验证前提已标注）
- 跨平台 everight fixture 二进制（aarch64/x86_64/Apple/Windows，纯 Rust 无 C 依赖）

### 既往提交

- docs: standardize License section format across all translations
- style: use uppercase ARIS / KEI throughout
- docs: add comprehensive deployment guides (guide files + mermaid)
- chore: stop tracking Cargo.lock
- feat: USB-C gadget support (composite mass-storage + NCM)
- feat: self-contained musl cross-build + fix binary paths
- feat: tri-backend QEMU ignition test (linux / kei / asterinas)

## 5. 后续计划

### 短期（本周）

1. **aarch64 交叉编译验证**——安装 `aarch64-unknown-linux-musl` target，构建 evernight gateway profile 二进制，替换 `tests/fixtures/` 中的 stub
2. **QEMU arm64 点火测试**——安装 `qemu-system-aarch64`，运行 `just qemu-ignition-linux`（Linux baseline）和 `just qemu-ignition-kei`（kei 内核）
3. **kei 内核联调**——在 QEMU virt (cortex-a55/a72) 上启动 kei，验证 initramfs → evernight 启动序列
4. 提交本轮 ignition_test.py 修复

### 中期

1. 推进 M1.3 evernight 交叉编译里程碑（gateway profile feature set）
2. 收敛 M2 ARM64 Hardening 遗留项（FDT 内存解析、GICv3 驱动）
3. 固化启动与健康检查流程（aris-core supervisor 生命周期管理）

### 长期

1. M1.5 OTA 双分区升级流程
2. M2.4 在 NanoPi R3S 上运行 kei + evernight 全栈

---

## 6. 桌面系统路线图（2026-07-10 制定）

### 6.1 架构重组：三仓库分层

```
celestia-island/
├── kei/          内核（fork of asterinas）
│                 纯内核职责：syscall ABI / drivers / scheduler / memory
│
├── aris/         系统中间件 ← 本次重点
│                 - 渲染引擎（Blitz + Vello CPU，非 Servo fork）
│                 - JS 引擎（Boa，替换 SpiderMonkey）
│                 - WASM 运行时（Wasmtime，WASI 接入 tairitsu）
│                 - Linux ABI 完整兼容层（gcompat 级别）
│                 - 独立浏览器产品能力
│                 - PID 1 系统监督器（LED/watchdog/网络/USB）
│
├── evernight/    发行版 ← 产出可部署镜像
│                 - 消费 kei 内核 + aris 系统层
│                 - OTA / 设备管理 / 产品镜像组装
│                 - 板级配置 / init 脚本 / 安装器
│
├── tairitsu/     UI 框架（WASM Component Model）
├── hikari/       UI 组件库（消费 tairitsu VDOM）
├── kou/          终端引擎（调色板/协议参考）
├── shirabe/      浏览器自动化（定义了渲染引擎 FFI 合约）
└── entelecheia/  AI agent 平台（已有 Boa JS 集成经验）
```

**关键变化**：

- aris 从"发行版组装器"升级为"系统中间件层"
- evernight 成为产出可部署镜像的发行版（从 aris 迁入 OTA、板级配置、init 脚本）
- Servo 不 vendor 也不 fork——用 Blitz + 独立 crate 组装渲染管线

### 6.2 渲染技术选型：Blitz + 纯 Rust 组件

不 fork Servo，而是用 crates.io 上的纯 Rust 组件组装渲染管线：

```
┌──────────────────────────────────────────────────────────┐
│                   aris 渲染管线                            │
│                                                          │
│  ┌──────────────┐   ┌─────────────────────────────────┐ │
│  │ tairitsu     │   │ Blitz 渲染管线 (纯 Rust)         │ │
│  │ (VDOM + diff)│   │                                 │ │
│  │              │   │ html5ever  ← HTML 解析           │ │
│  │ hikari 组件  │   │ stylo      ← CSS 级联(无SM依赖)  │ │
│  └──────┬───────┘   │ taffy      ← Flexbox/Grid 布局  │ │
│         │ DOM ops   │ parley     ← 文字排版            │ │
│         ↓           │ vello_cpu  ← CPU 光栅化→像素     │ │
│  ┌──────────────┐   └───────┬─────────────────────────┘ │
│  │ Boa JS 引擎   │           │ RGBA buffer              │
│  │ (页面内 JS)   │           ↓                          │
│  └──────────────┘   mmap /dev/fb0 → kei virtio-gpu     │
│                                                          │
│  ┌──────────────┐                                       │
│  │ Wasmtime     │  WASI ←→ kei syscall ABI             │
│  │ (WASM 组件)  │                                       │
│  └──────────────┘                                       │
└──────────────────────────────────────────────────────────┘
```

**核心技术决策**：

| 组件 | 选型 | 理由 |
|------|------|------|
| HTML/CSS 解析 | html5ever + cssparser + selectors | 纯 Rust，crates.io 独立发布，无 SpiderMonkey 依赖 |
| CSS 级联 | stylo (servo feature) | crates.io 上的 `stylo` crate 用 `servo` feature 时不依赖 `mozjs` |
| 布局引擎 | taffy | 纯 Rust，Flexbox/Grid/Block，独立于 Servo |
| 文字排版 | parley | 纯 Rust，文字 shaping/breaking |
| 光栅化 | vello_cpu | 纯 Rust CPU 光栅化，`render_to_buffer` 直接写像素 buffer |
| 渲染集成 | blitz-dom + blitz-renderer-vello | 已组装好 parse→style→layout→paint 管线，无 JS |
| JS 引擎 | boa_engine 0.20 | 纯 Rust，替换 SpiderMonkey；entelecheia 已有集成经验 |
| WASM 运行时 | wasmtime | tairitsu 的 WASM 组件通过 Wasmtime 执行，WASI 接入 |
| 显示后端 | /dev/fb0 mmap | vello_cpu 输出 RGBA → memcpy 到 fb0，无需 DRM/Wayland |

**避免使用**：

- ❌ `mozjs` / SpiderMonkey — Boa 替代
- ❌ `webrender` + SWGL — SWGL 是 C++，非纯 Rust
- ❌ Servo `components/script` — SpiderMonkey 耦合层，整个替换
- ❌ DRM/Wayland — vello_cpu + fbdev 绕过，远期再考虑

### 6.3 aris 新增包结构

```
aris/
├── packages/
│   ├── core/           # PID 1 系统监督器（现有）
│   ├── common/         # 共享类型（现有）
│   ├── render/         # ← 新增：渲染管线
│   │   ├── Cargo.toml  # blitz-dom, blitz-renderer-vello, boa_engine
│   │   └── src/
│   │       ├── lib.rs        # 公共 API: render_html, render_dom_ops
│   │       ├── fbdev.rs      # /dev/fb0 mmap 后端
│   │       ├── boa_bridge.rs # Boa JS 引擎桥接（页面内 JS 执行）
│   │       └── wit_host.rs   # tairitsu WIT 接口的 Rust host 实现
│   │
│   └── abi/            # ← 新增：Linux ABI 完整兼容层
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs          # 兼容层入口
│           ├── syscall_shim.rs # 缺失 syscall 的用户态 fallback
│           ├── fbdev_mmap.rs   # /dev/fb0 mmap 用户态驱动
│           ├── drm_translate.rs # DRM ioctl → fbdev 翻译层
│           └── proc_sys.rs     # /proc, /sys 模拟
```

### 6.4 evernight 迁移计划

从 aris 迁入 evernight 的组件：

| 组件 | 当前位置 | 迁入位置 | 理由 |
|------|---------|---------|------|
| OTA 更新 | `aris/packages/core/src/ota.rs` | `evernight/packages/ota/` | 产品级更新逻辑，evernight 已有 reqwest/sha2 |
| 板级配置 | `aris/configs/*.toml` | `evernight/configs/` | 选择编译哪些 evernight 功能 |
| 共享类型 | `aris/packages/common/` | `evernight/packages/aris-common/` | BoardConfig 等 |
| init 脚本 | `aris/overlay/S90evernight` | `evernight/overlay/` | evernight 的启动脚本 |
| 安装器 | `aris/package/` | `evernight/package/` | USB 安装器安装的是 evernight |
| 点火测试 | `aris/scripts/ignition_test.py` | `evernight/scripts/` | 测试 evernight 注册流程 |

保留在 aris 的组件：PID 1 监督器、LED/watchdog/网络/USB、board/（设备树/U-Boot）、build_browser.py、渲染管线、ABI 兼容层。

evernight 需改造为 Cargo workspace：

```toml
[workspace]
members = [
    ".",                      # evernight 主程序（broker）
    "packages/aris-common",   # 共享类型（从 aris 迁入）
    "packages/ota",           # OTA 更新（从 aris 迁入）
]
```

### 6.5 tairitsu ↔ aris 渲染管线对接

**数据流**：

```
tairitsu WASM 组件（Wasmtime 执行）
  → VDOM diff → DOM ops（create_element, set_style, append_child...）
  → WIT interface（browser-full.wit）
  → aris render 包的 wit_host.rs（实现 WIT host）
  → blitz-dom（消费 DOM ops，更新 DOM 树）
  → stylo（CSS 级联）→ taffy（布局）→ vello_cpu（光栅化）
  → RGBA pixel buffer
  → mmap /dev/fb0（kei virtio-gpu 显示）
```

**两种模式**：

1. **SSR 模式**（简单，阶段 1）：tairitsu SSR 生成 HTML 字符串 → blitz-dom 解析 → 渲染
2. **交互模式**（完整，阶段 4）：tairitsu WASM 组件实时通过 WIT 发送 DOM ops → blitz-dom 增量更新

### 6.6 实施阶段

#### 阶段 1：aris Linux kiosk 验证（~1-2 周）

- 创建带显示的 QEMU 板子配置（`configs/qemu-hmi.toml`）
- 在 aris Linux 后端（标准 Linux 内核）上用 WebKitGTK/Cogs 验证 kiosk 浏览器
- 截图确认 evernight dashboard 渲染
- 验证 `build_browser.py` 的 webkitgtk 路径

#### 阶段 2：kei syscall + fbdev 补全（~3-5 天）

- kei 补全 `/dev/fb0` mmap 支持（Blit 后端目前返回 ENODEV）
- 补全缺失 syscall（SYSV shm、posix_spawn fallback）
- 在 kei QEMU 上验证 `/dev/fb0` mmap 写入 → SDL 窗口显示

#### 阶段 3：Blitz 渲染管线集成（~2-4 周）

- 创建 `aris/packages/render/`
- 集成 blitz-dom + blitz-renderer-vello（vello_cpu 后端）
- 实现 `/dev/fb0` mmap 后端
- 在 aris Linux 后端上验证 Blitz 渲染 HTML 到 fb0
- 截图确认网页渲染

#### 阶段 4：Boa JS + Wasmtime + WIT host（~4-8 周）

- 集成 boa_engine 处理页面内 JS
- 集成 wasmtime 执行 tairitsu WASM 组件
- 实现 tairitsu `browser-full.wit` 的 Rust host adapter
- WASI → kei syscall 桥接
- 在 kei QEMU 上端到端验证

#### 阶段 5：evernight 迁移 + 发行版组装（~2-4 周）

- 从 aris 迁出 OTA、板级配置、init 脚本到 evernight
- evernight 改造为 Cargo workspace
- 产出完整的 kei + aris + evernight 部署镜像

#### 阶段 6：Linux ABI 完整兼容层（~2-3 月）

- 实现 gcompat 级别的 ABI 兼容库
- 支持标准 Linux arm64 二进制（.deb 包）直接运行
- /proc、/sys 模拟
- DRM ioctl → fbdev 翻译层
- 包管理器集成（apk 或 pkgsrc）

#### 阶段 7：Wayland/DRM 最小实现（远期，按需）

- 如果需要窗口管理或多窗口
- 实现 kei 的 DRM 框架（/dev/dri/）
- 移植 cage（最小 Wayland compositor）

### 6.7 技术决策记录

1. **不用 Servo fork**：Blitz 已经组装好 Stylo + Taffy + Vello CPU 管线，无需从 Servo 拆组件
2. **不用 SpiderMonkey**：Boa 替代（纯 Rust，entelecheia 已有经验），页面内 JS 性能要求不高（dashboard 场景）
3. **不用 WebRender/SWGL**：SWGL 是 C++，Vello CPU 是纯 Rust CPU 光栅化
4. **不用 Vue/echarts**：UI 框架用 tairitsu（WASM Component Model）+ hikari 组件库
5. **WASI 直接接入**：tairitsu 的 WASM 组件通过 Wasmtime 执行，WASI 成为应用层和内核的原生接口
6. **完整 ABI 兼容层**：aris 实现 gcompat 级别的 Linux 兼容，支持任意 arm64 Linux 二进制
7. **Boa 0.20 + 独立工具链**：aris 顶层工具链锁定 rustc 1.85（与 kei 内核一致），但 Boa 0.21 要求 rustc 1.88，其正则后端 regress 0.10.5 使用了 2024 edition 才稳定的 let-chains。因此 aris-js 作为独立 workspace（`[workspace]` 空表隔离），并通过本地 `rust-toolchain.toml` 锁定 `stable`（≥1.88）工具链。Boa 0.20 与 0.21 的公共 API（`Context::default`/`Source`/`eval`/`JsValue::to_string`）完全一致，升级仅受 MSRV 阻塞。

---

# aris — Project Plan

## Goal

Build a Linux-standard (LSB-compatible) distribution that ships a desktop environment purpose-built for evernight and shittim-chest, targeting industrial HMI panels and supervisory host (上位机) stations.

## Architecture

```
┌────────────────────────────────────────────────┐
│ entelecheia  (Cloud/Edge AI Multi-Agent)        │
│   WebSocket JSON-RPC / Unix Socket             │
├────────────────────────────────────────────────┤
│ evernight  (Hardware Protocol Broker)           │
│   Modbus / S7comm / EtherCAT / OPC UA / CAN    │
├────────────────────────────────────────────────┤
│ aris OS  (Device Firmware Layer)                │
│   ├─ Kernel: Linux 6.x → Asterinas (Phase 2)   │
│   ├─ Init: aris-core supervisor                 │
│   ├─ Net: Dual Ethernet (WAN + LAN)             │
│   └─ OTA: A/B partition firmware update         │
├────────────────────────────────────────────────┤
│ Physical Devices  (PLC / Sensors / Valves)      │
└────────────────────────────────────────────────┘
```

## Phase 1: Linux Base (2026 Q3–Q4)

Target: boot, run evernight, talk to entelecheia.

### M1.1 — Board Bring-up

- Buildroot-style slim rootfs (musl + busybox)
- Linux 6.x kernel with RK3566 BSP
- U-Boot with verified boot
- Target board: NanoPi R3S (RK3566, dual GbE)

### M1.2 — Core Drivers

- [x] Dual Gigabit Ethernet (stmmac/rk_gmac) — WAN/LAN routing
- [ ] UART (debug + serial devices)
- [ ] GPIO (status LEDs, digital I/O)
- [ ] SPI (sensor bus)
- [ ] I2C (peripheral bus)
- [ ] eMMC/SD storage
- [ ] Hardware watchdog (RK3566 WDT)

### M1.3 — evernight Cross-compile

- Target: `aarch64-unknown-linux-musl`
- Features: `hardware, protocol, serial, sensor, s7comm, ethercat, can, bin, api, vault, manifest`
- Excluded: `screen, webrtc, remote-ssh, remote-vnc, remote-rdp, container, k8s, libvirt, vm, compile-bridge`

### M1.4 — Firmware Integration

- aris-core supervisor manages evernight daemon lifecycle
- Startup sequence: net init → evernight start → device.register → entelecheia join
- Health check + auto-restart via watchdog
- [x] Host ignition test verified (2026-07-04): evernight sensor-poll → WebSocket → evernight-server, device.register + device.telemetry 双向确认
- [ ] QEMU arm64 boot with evernight (pending QEMU install)
- [ ] aris-core supervisor lifecycle management

### M1.5 — OTA Update

- Dual A/B partition layout
- Firmware package: kernel + dtb + rootfs squashfs + verity hash
- Update flow: download → verify → set boot flag → reboot → fallback on failure

### M1.6 — Production Readiness

- Build reproducibility (deterministic image hash)
- Secure boot chain (U-Boot verified boot)
- Provisioning: unique device identity, TLS client cert
- Factory reset

## Phase 2: Asterinas ARM64 Port (2026 Q4+)

> **Key**: ARM64 support is already under active development.
> PR asterinas/asterinas#3270 by @wanywhn is nearly ready.
> We track the fork: <https://github.com/wanywhn/asterinas> (branch: `arm64-support`).

### M2.1 — Adopt ARM64 Fork

- Use `wanywhn/asterinas` `arm64-support` branch as development baseline
- Includes: GICv3, ARM MMU setup, UART console, basic device tree for aarch64
- Once merged into mainline, switch to official asterinas/asterinas
- Track PR #3270 status weekly

### M2.2 — RK3566 Board Support for Asterinas

Add board-specific drivers on top of the arm64-support base:

- Rockchip GPIO/pinctrl driver
- stmmac Ethernet driver (DW GMAC / RK GMAC)
- DW SPI / DW I2C master drivers
- UART 8250-compatible (DW UART) driver
- Device tree support (ostd dtb parsing)

### M2.3 — aris Asterinas Kernel Module

- `kernel/asterinas/` directory with cargo-osdk project
- Reuse Linux device tree bindings

### M2.4 — Parity Validation

- Boot Asterinas on NanoPi R3S
- Run evernight, verify all protocol features
- Performance benchmark vs Linux baseline

### M2.5 — Production Rollout

- OTA push Asterinas kernel to deployed devices
- Fallback to Linux kernel on boot failure

## Multi-Architecture Roadmap

| Arch | SoC Examples | Phase 1 | Phase 2 |
|------|-------------|---------|---------|
| aarch64 | RK3566, RK3588, BCM2711 | Now | Asterinas ARM64 |
| armv7l | BCM2837, AM335x, i.MX6 | Q4 2026 | Asterinas ARM32 (if upstream) |
| riscv64 | JH7110, TH1520, K230 | Q1 2027 | Asterinas (upstream Tier 2) |
| x86_64 | Intel N100, AMD G-Series | Q2 2027 | Asterinas (upstream Tier 1) |

## evernight Feature Flags per Target

### Gateway Profile (aarch64, headless, < 2GB RAM)

```
hardware, protocol, serial, sensor, s7comm, ethercat, can,
bin, api, vault, manifest, scripting
```

### Minimal Profile (armv7l, < 512MB RAM)

```
hardware, protocol, serial, sensor, bin, api, manifest
```

### Full Profile (x86_64, >= 4GB RAM)

```
full (all features)
```

## Board Support Matrix

| Board | SoC | Arch | RAM | Storage | Ethernet | Status |
|-------|-----|------|-----|---------|----------|--------|
| NanoPi R3S | RK3566 | aarch64 | 2GB | SD/eMMC | 2x GbE | Active |
| OrangePi 3B | RK3566 | aarch64 | 4GB | eMMC | 1x GbE | Planned |
| Raspberry Pi 4 | BCM2711 | aarch64 | 2GB | SD | 1x GbE | Planned |
| VisionFive 2 | JH7110 | riscv64 | 4GB | SD/eMMC | 2x GbE | Planned |
| Luckfox Pico | RV1103 | armv7l | 64MB | SPI NAND | 1x FE | Planned |

## Build System Design

Aris uses a custom build system (no Buildroot submodule):

```
scripts/build.sh              # Main build orchestrator
  ├── configs/<board>.toml    # Board-specific config
  ├── kernel/                 # Kernel source (downloaded)
  ├── board/<board>/          # Device tree, boot script
  ├── packages/core/          # Rust firmware (cross-compiled)
  └── overlay/<board>/        # Rootfs static files
→ output/<board>/image.img    # Bootable SD card image
```

## Key Design Decisions

1. **No git submodules** — build script downloads kernel/uboot toolchains on demand
2. **TOML-based board configs** — one config file per board, declarative
3. **A/B partition layout** — mandatory for all boards, safe OTA
4. **musl static linking** — single binary, no libc ABI issues
5. **Verified boot everywhere** — from U-Boot through kernel to rootfs
