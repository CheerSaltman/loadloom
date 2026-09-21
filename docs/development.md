# 开发、验收与发布

本文档面向维护者，包含本地开发环境、质量门禁、本机验收命令与发布步骤。

## 开发环境

| 依赖 | 版本 / 说明 |
| --- | --- |
| Rust | 由 `rust-toolchain.toml` 固定为 `stable`，含 `rustfmt` + `clippy` |
| Node.js | ≥ 20（开发机使用便携版 Node v22） |
| WebView2 Runtime | Windows 上运行桌面壳所必需（Win11 自带） |
| Git for Windows | 提供 `git` 与 Git Credential Manager |

## 常用命令

```bash
npm install                                # 安装前端依赖
npm run typecheck                          # tsc --noEmit
npm run build                              # 产出前端 dist/
npm run desktop:dev                        # 开发模式：热重载原生窗口
npm run desktop:build                      # 打包：exe + NSIS + MSI
```

## 质量门禁（提交前必过）

```bash
cargo fmt --all --check                                          # 格式
cargo clippy --locked -p loadloom-core --all-targets -- -D warnings
cargo test  --locked -p loadloom-core                             # 18 个测试，不需要图形环境
cargo run   --locked --example headless_smoke -p loadloom-core     # 示例可运行
```

任何一步失败都不要进入下一步。CI（`.github/workflows/ci.yml`）会把同样的门禁再跑一遍。

## 测试构成

| 套件 | 数量 | 覆盖什么 |
| --- | --- | --- |
| `engine`（单元测试） | 6 | 限速器、指标聚合、自动停止、阶段迁移 |
| `contract_wire` | 7 | 解析 `src/bindings.ts` 逐字段比对 serde 实产 JSON |
| `headless_e2e` | 5 | 本地伪 origin 跑真实打流，含「裸线程无 Tokio 运行时」启动 |

`crates/loadloom-core/benches/throughput.rs` 是基准台，默认被 `#[ignore]` 门控，需要显式启用：

```bash
cargo test --release -p loadloom-core --bench throughput -- --ignored --nocapture
```

> 回环地址上的吞吐数字只能说明引擎能否运行、速率是否随并发上升，不能代表真实目标的吞吐 —— 瓶颈通常在测试机自身或回环协议栈上。

## 本机验收

```bash
scripts/verify_launch.ps1       # 启动验收：确认是真实窗口 HWND、进程不监听任何端口
scripts/audit_residue.ps1       # 遗留依赖审计：确认核心依赖闭包内没有 GUI 库
scripts/throughput_origin.js    # 外部参照 origin，排除本地 harness 对基准的干扰
scripts/make_icon.mjs           # 图标生成
```

## 发布流程

1. 确认质量门禁全绿、`CHANGELOG.md` 已写好当版条目。
2. 更新各处的版本号（`Cargo.toml` 工作区 `version`、`package.json`），提交。
3. 打标签并推送：
   ```bash
   git tag -a v0.4.0 -m "LoadLoom v0.4.0"
   git push origin v0.4.0
   ```
4. 推送 tag 后无需人工出包：`.github/workflows/release.yml` 会在 tag 推送时自动跑核心测试与前端类型检查、
   构建并上传三个产物到对应 Release。

   > 手工兜底（CI 不可用时）：在本机执行 `npm run desktop:build`，再按上表产物上传到 Release。

   | 产物 | 路径 |
   | --- | --- |
   | 免安装 exe | `target/release/loadloom-desktop.exe` |
   | NSIS 安装包 | `target/release/bundle/nsis/LoadLoom_<版本>_x64-setup.exe` |
   | MSI 安装包 | `target/release/bundle/msi/LoadLoom_<版本>_x64_en-US.msi` |


## 本机环境注意事项

- **不要通过目录联接（junction）构建。** 若把项目目录用 `mklink /J` 映射成一个 ASCII 路径，
  从该路径运行 `vite build` 会失败：
  ```text
  [vite:build-html] The "fileName" or "name" properties of emitted chunks and assets
  must be strings that are neither absolute nor relative paths,
  received "../<真实目录名>/loadloom/index.html"
  ```
  原因是 Vite 会把路径解析回真实路径，再计算相对路径时多出 `..`，Rollup 直接拒绝。
  请始终在项目真实路径下构建，联接仅可用于 `git` 等工具。
- **改动目录结构后需要 `cargo clean`。** `target/` 中会缓存绝对路径，移动项目目录后会看到
  形如「找不到 app_hide.toml」的构建脚本报错。
- **PowerShell 执行策略。** 若策略禁止运行脚本，`npm.ps1` 会被拦截，改用 `npm.cmd`。
