//! 桌面壳入口。
//!
//! 本文件**只是启动器**：真正的业务全部在 `loadloom-core` 中。
//! 发布版以 GUI 子系统运行，不弹控制台窗口。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    loadloom_desktop_lib::run();
}
