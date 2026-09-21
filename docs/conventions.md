# 仓库规范对照

本仓库的目录与文件组织不是凭手感摆的 —— 下表每条都能指回官方文档。

| 条目 | 依据 | 本仓库对应物 |
| --- | --- | --- |
| 包布局 `src/tests/benches/examples` | Cargo Book · Package Layout | `crates/traffic-core/` 四分目录；基准不混进 `tests/` |
| 清单元数据与工作区继承 | Cargo Book · Manifest / Workspaces | `[workspace.package]` + 各 crate 的 `xxx.workspace = true` |
| 统一 lint 策略 | Cargo Book · `[lints]` | `[workspace.lints.rust/clippy]` + 成员 `[lints] workspace = true` |
| 禁止 unsafe | Rust 参考 · `unsafe_code` lint | `[workspace.lints.rust] unsafe_code = "forbid"` |
| 收窄无用公开项 | Rust 参考 · `unreachable_pub` lint | 开启为 `warn`，配合 CI 的 `-D warnings` 生效 |
| 固定编译器 | rustup Book · Toolchain Overrides | `rust-toolchain.toml`（`stable` + rustfmt/clippy，`profile = "minimal"`） |
| 格式化一致性 | rustfmt 配置文档 | `rustfmt.toml`，只用 stable 选项，CI 跑 `--check` |
| 缩进 / 换行统一 | EditorConfig 规范 | `.editorconfig` |
| 换行与二进制归属 | Git · gitattributes | `.gitattributes`（文本 LF、图标与安装包标 binary） |
| 变更记录 | Keep a Changelog 1.1.0 + SemVer 2.0.0 | `CHANGELOG.md` |
| 贡献流程 | GitHub · 社区健康文件 | `CONTRIBUTING.md` |
| 漏洞报告 | GitHub · Security policy | `SECURITY.md`（含密钥泄露处置流程） |
| 许可声明 | SPDX 标识 + Cargo `license` | `LICENSE`（MIT）+ `license.workspace = true` |
| 持续集成 | GitHub Actions 文档 | `.github/workflows/ci.yml` |
| 依赖更新 | Dependabot 配置文档 | `.github/dependabot.yml`（cargo ×2 + npm + actions） |

## 关于 `rust-version`

`Cargo.toml` 中的 `rust-version = "1.98"` 是**实测**下限，不是随手写的数字。
若想放宽，请先用 `cargo msrv --bisect` 测出真实 MSRV 再改 —— 否则会在用户机器上
以「编译器太旧」的形态炸掉，而不是在 CI 里。
