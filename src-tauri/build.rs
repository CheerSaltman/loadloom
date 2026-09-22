fn main() {
    // 构建标识：把当前 commit 短哈希注入二进制。
    //
    // 版本号保持不变（热更新）时，这是唯一能区分「同一版本号的不同构建」的字段：
    // 支持时用户报出构建号，就能确认他手里到底是哪一次发布的产物；日志环境头
    // 与诊断文本都会带上它。
    let build_id = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=LOADLOOM_BUILD_ID={build_id}");
    // 提交、切分支、打标签都要重新注入，避免沿用上一次的构建号。
    // （HEAD 只存 ref 名，真正的提交哈希在 refs/ 下；两个都要看。）
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
    println!("cargo:rerun-if-changed=../.git/refs/tags");

    tauri_build::build()
}
