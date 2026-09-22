//! Windows 实现：`GetIfTable2` / `FreeMibTable`（`iphlpapi.dll`）。
//!
//! 为什么用 `GetIfTable2` 而不是 WMI / `Get-NetAdapter`：
//!
//! * 它是**一次系统调用返回全部接口**，500ms 采样一次的开销可以忽略；
//! * 不需要管理员权限、不启动额外的进程、不需要 COM/WMI 服务；
//! * 返回的字段就是驱动上报的原始 MIB 计数器，没有中间层做二次加工，
//!   「界面上的数字」与「驱动里的数字」之间不存在解释空间。
//!
//! 本文件是全工作区**唯一**允许 unsafe 的地方（见 crate 文档）。

// 唯一的放行点：FFI 调用本身必须 unsafe，其余代码一律禁止。
#![allow(unsafe_code)]

use std::ffi::c_void;

use windows_sys::Win32::Foundation::NO_ERROR;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
};

use crate::{classify, media_label, AdapterClass, AdapterFacts, Counters, NicError, RawAdapter};

/// `MIB_IF_TABLE2` 的所有权包装。
///
/// 表由 `iphlpapi` 分配，**必须**用 `FreeMibTable` 归还。用 RAII 包住是为了让
/// 「转换中途提前返回 / 出错」也照样释放 —— 手动释放的写法在加分支的那一天就会漏。
struct TableGuard(*mut MIB_IF_TABLE2);

impl Drop for TableGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 指针来自 GetIfTable2 成功返回，且在 TableGuard 里只释放一次。
            unsafe { FreeMibTable(self.0 as *const c_void) };
        }
    }
}

pub fn list_adapters() -> Result<Vec<RawAdapter>, NicError> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: 传入的是指向局部变量的有效指针；成功时 iphlpapi 写入一块新分配的表。
    let status = unsafe { GetIfTable2(&mut table) };
    if status != NO_ERROR {
        return Err(NicError::call_failed(status));
    }
    if table.is_null() {
        return Err(NicError::malformed(
            "GetIfTable2 报告成功，却返回了空表指针",
        ));
    }
    let guard = TableGuard(table);

    // SAFETY: 成功返回时表头之后紧跟着 `NumEntries` 行 `MIB_IF_ROW2`（由 SDK 保证
    // 的变长数组布局），元素类型来自同一份 windows-sys 元数据，布局一致。
    let rows: &[MIB_IF_ROW2] = unsafe {
        let head = guard.0;
        let count = (*head).NumEntries as usize;
        if count == 0 {
            &[]
        } else {
            std::slice::from_raw_parts((*head).Table.as_ptr(), count)
        }
    };

    let mut adapters: Vec<RawAdapter> = rows.iter().map(to_adapter).collect();
    // 固定顺序：物理网卡在前，其次按名字。顺序稳定的列表才能被用户「一眼看出
    // 哪块网卡换了位置」，也让日志里的对比不会因枚举顺序抖动。
    adapters.sort_by(|left, right| {
        class_rank(left.class)
            .cmp(&class_rank(right.class))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(adapters)
}

/// 排序权重（仅用于展示顺序，不参与任何过滤决策）。
fn class_rank(class: AdapterClass) -> u8 {
    match class {
        AdapterClass::Physical => 0,
        AdapterClass::Other => 1,
        AdapterClass::Virtual => 2,
        AdapterClass::Tunnel => 3,
        AdapterClass::Loopback => 4,
    }
}

fn to_adapter(row: &MIB_IF_ROW2) -> RawAdapter {
    let flags = row.InterfaceAndOperStatusFlags._bitfield;
    let hardware = flags & 0b0000_0001 != 0;
    let connector_present = flags & 0b0000_0100 != 0;
    let not_media_connected = flags & 0b0001_0000 != 0;
    let endpoint = flags & 0b1000_0000 != 0;

    let name = wide_to_string(&row.Alias);
    let description = wide_to_string(&row.Description);
    let if_type = row.Type;
    let media_type = as_u32(row.MediaType);
    let tunnel_type = as_u32(row.TunnelType);
    // SAFETY: NET_LUID_LH 是联合体，`Value` 是它的完整 64 位视图（无符号、无填充）。
    let luid = unsafe { row.InterfaceLuid.Value };

    RawAdapter {
        id: format!("luid-{luid:016x}"),
        index: row.InterfaceIndex,
        luid,
        class: classify(&AdapterFacts {
            alias: &name,
            description: &description,
            if_type,
            media_type,
            tunnel_type,
            hardware,
            endpoint,
        }),
        media: media_label(if_type, media_type),
        name,
        description,
        mac: physical_address(row),
        mtu: row.Mtu,
        oper_status: as_u32(row.OperStatus),
        admin_status: as_u32(row.AdminStatus),
        connect_state: as_u32(row.MediaConnectState),
        hardware,
        connector_present,
        not_media_connected,
        transmit_speed_bps: row.TransmitLinkSpeed,
        receive_speed_bps: row.ReceiveLinkSpeed,
        counters: Counters {
            in_octets: row.InOctets,
            in_ucast_pkts: row.InUcastPkts,
            in_nucast_pkts: row.InNUcastPkts,
            in_discards: row.InDiscards,
            in_errors: row.InErrors,
            in_unknown_protos: row.InUnknownProtos,
            out_octets: row.OutOctets,
            out_ucast_pkts: row.OutUcastPkts,
            out_nucast_pkts: row.OutNUcastPkts,
            out_discards: row.OutDiscards,
            out_errors: row.OutErrors,
            out_queue_len: row.OutQLen,
        },
    }
}

/// `[u16; 257]` 固定缓冲 -> `String`（按第一个 NUL 截断；非法代理对按替换字符处理）。
fn wide_to_string(buffer: &[u16]) -> String {
    let length = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

/// 物理地址按 `AA-BB-CC-DD-EE-FF` 输出；无线网卡没有 MAC 时返回空串。
fn physical_address(row: &MIB_IF_ROW2) -> String {
    let length = (row.PhysicalAddressLength as usize).min(row.PhysicalAddress.len());
    if length == 0 {
        return String::new();
    }
    row.PhysicalAddress[..length]
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join("-")
}

/// SDK 的枚举字段都是 `i32`，负值只可能是内存被改写；一律按 0（未知）处理，
/// 绝不让负数以 `as u32` 变成 42 亿这种会污染统计的值。
fn as_u32(value: i32) -> u32 {
    value.max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_stop_at_the_nul_terminator() {
        let mut buffer = [0_u16; 257];
        for (index, unit) in "以太网".encode_utf16().enumerate() {
            buffer[index] = unit;
        }
        assert_eq!(wide_to_string(&buffer), "以太网");
    }

    #[test]
    fn wide_strings_never_panic_on_garbage() {
        // 未终止的缓冲区（驱动没写 NUL）必须按全长处理，不能越界读。
        let buffer = [0x41_u16; 257];
        assert_eq!(wide_to_string(&buffer).chars().count(), 257);
        // 落单的代理项（半个 emoji）不能 panic。
        let mut lone = [0_u16; 257];
        lone[0] = 0xD800;
        assert!(!wide_to_string(&lone).is_empty());
    }

    #[test]
    fn negative_enum_values_become_unknown_not_four_billion() {
        assert_eq!(as_u32(-1), 0);
        assert_eq!(as_u32(1), 1);
    }
}
