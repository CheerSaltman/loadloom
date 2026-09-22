//! 非 Windows 平台的降级实现。
//!
//! 明确返回「不支持」，而不是返回一个空列表 —— 空列表会被上层显示成
//! 「本机没有网卡」，而事实是**这块平台还没实现**。两者的排障方向完全不同。

use crate::{NicError, RawAdapter};

pub fn list_adapters() -> Result<Vec<RawAdapter>, NicError> {
    Err(NicError::unsupported(format!(
        "网卡计数器读取目前只有 Windows 实现（当前平台 {}）",
        std::env::consts::OS
    )))
}
