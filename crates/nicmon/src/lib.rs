//! 只读网卡计数器采集层 —— 消费级能力边界内的最大集合。
//!
//! ## 为什么单独一个 crate
//!
//! 工作区把所有成员钉在 `unsafe_code = "forbid"` 上（见根 `Cargo.toml`），而读取
//! 网卡计数器的唯一途径是 Windows 的 `GetIfTable2` —— 一个必须走 FFI 的系统调用。
//! `forbid` 无法在 crate 内部被 `allow` 覆盖，所以把 FFI 隔离进**唯一**一个不继承
//! 该 lint 的 crate：本 crate 自己声明 `unsafe_code = "deny"`，只在 [`windows`]
//! 模块顶部放行一次。其余代码与其它 crate 一样禁止 unsafe，隔离边界是可见的。
//!
//! ## 能拿到什么 / 拿不到什么（诚实边界）
//!
//! 拿得到（`GetIfTable2` 公开的 MIB 计数器，普通用户权限即可读，无需驱动、无需管理员）：
//!
//! * 协商链路速率（收 / 发）、连接状态、运行状态、介质类型；
//! * 收发字节数、单播 / 非单播包数、丢弃数、错误数、未知协议数；
//! * **发送队列长度** `OutQLen`（当前待发送的包数）。
//!
//! 拿不到（**不做**，也绝不编造一个看起来像数据的近似值）：
//!
//! * **网卡温度** —— 消费级网卡驱动不暴露温度传感器，只有少数服务器级网卡带
//!   `NDIS_OID_TEMPERATURE` 之类的私有 OID，且厂商不保证实现；
//! * **收发缓冲区占用率 / 剩余槽位** —— 属于驱动内部实现，Windows 不对外公开；
//!   唯一公开的队列深度就是 `OutQLen`（单位是包，不是缓存字节）；
//! * 光模块收发光功率、PHY 误码率、信噪比 —— 只存在于厂商私有 IOCTL。

#[cfg(not(windows))]
mod unsupported;
#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
use unsupported as backend;
#[cfg(windows)]
use windows as backend;

use std::fmt;

// ---------------------------------------------------------------------------
// 平台常数（与 Windows SDK / IANA ifType 对齐，非 Windows 也会被纯函数测试引用）
// ---------------------------------------------------------------------------

/// `IF_OPER_STATUS`：链路可用。
pub const IF_OPER_UP: u32 = 1;
/// `IF_OPER_STATUS`：链路断开。
pub const IF_OPER_DOWN: u32 = 2;
/// `IF_OPER_STATUS`：自检中（网卡刚上电，计数器还不可信）。
pub const IF_OPER_TESTING: u32 = 3;
/// `IF_OPER_STATUS`：状态未知。
pub const IF_OPER_UNKNOWN: u32 = 4;
/// `IF_OPER_STATUS`：休眠（等待外部事件，例如拨号、Wi-Fi 关联）。
pub const IF_OPER_DORMANT: u32 = 5;
/// `IF_OPER_STATUS`：设备不存在（已拔出 / 已禁用）。
pub const IF_OPER_NOT_PRESENT: u32 = 6;
/// `IF_OPER_STATUS`：下层（物理层）不可用。
pub const IF_OPER_LOWER_LAYER_DOWN: u32 = 7;

/// `NET_IF_MEDIA_CONNECT_STATE`：未知。
pub const MEDIA_CONNECT_UNKNOWN: u32 = 0;
/// `NET_IF_MEDIA_CONNECT_STATE`：已连接。
pub const MEDIA_CONNECT_CONNECTED: u32 = 1;
/// `NET_IF_MEDIA_CONNECT_STATE`：未连接。
pub const MEDIA_CONNECT_DISCONNECTED: u32 = 2;

/// IANA `ifType`：以太网。
pub const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
/// IANA `ifType`：软件回环。
pub const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
/// IANA `ifType`：IEEE 802.11 无线。
pub const IF_TYPE_IEEE80211: u32 = 71;
/// IANA `ifType`：隧道。
pub const IF_TYPE_TUNNEL: u32 = 131;
/// IANA `ifType`：WiMAX。
pub const IF_TYPE_IEEE80216_WMAN: u32 = 237;
/// IANA `ifType`：3GPP（移动宽带）。
pub const IF_TYPE_WWANPP: u32 = 243;

/// `NDIS_MEDIUM`：802.3（以太网）。
pub const MEDIA_802_3: u32 = 0;
/// `NDIS_MEDIUM`：无线 WAN。
pub const MEDIA_WIRELESS_WAN: u32 = 9;
/// `NDIS_MEDIUM`：隧道。
pub const MEDIA_TUNNEL: u32 = 15;
/// `NDIS_MEDIUM`：原生 802.11。
pub const MEDIA_NATIVE_802_11: u32 = 16;
/// `NDIS_MEDIUM`：回环。
pub const MEDIA_LOOPBACK: u32 = 17;

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

/// 适配器类别。默认只监测 [`AdapterClass::Physical`]，其余类别由用户显式勾选后才纳入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterClass {
    /// 真实物理网卡（有线 / 无线 / 移动宽带）。
    Physical,
    /// 虚拟网卡（Hyper-V / VMware / VPN 客户端 / 环回适配器……）。
    Virtual,
    /// 软件回环。
    Loopback,
    /// 隧道（Teredo / ISATAP / 6to4 / WFP 隧道……）。
    Tunnel,
    /// 其它无法归类的接口（例如点对点端点）。
    Other,
}

impl AdapterClass {
    /// 线格式 / 持久化用的稳定标识（小驼峰，与前端 `NicAdapterClass` 对齐）。
    pub fn as_str(self) -> &'static str {
        match self {
            AdapterClass::Physical => "physical",
            AdapterClass::Virtual => "virtual",
            AdapterClass::Loopback => "loopback",
            AdapterClass::Tunnel => "tunnel",
            AdapterClass::Other => "other",
        }
    }

    /// 界面展示用中文标签。
    pub fn label(self) -> &'static str {
        match self {
            AdapterClass::Physical => "物理网卡",
            AdapterClass::Virtual => "虚拟网卡",
            AdapterClass::Loopback => "回环",
            AdapterClass::Tunnel => "隧道",
            AdapterClass::Other => "其它接口",
        }
    }

    /// 是否默认纳入监测。
    pub fn monitored_by_default(self) -> bool {
        matches!(self, AdapterClass::Physical)
    }
}

/// 累计计数器原值（64 位；驱动回绕 / 重插会把它清零）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub in_octets: u64,
    pub in_ucast_pkts: u64,
    pub in_nucast_pkts: u64,
    pub in_discards: u64,
    pub in_errors: u64,
    pub in_unknown_protos: u64,
    pub out_octets: u64,
    pub out_ucast_pkts: u64,
    pub out_nucast_pkts: u64,
    pub out_discards: u64,
    pub out_errors: u64,
    /// 发送队列当前积压的包数（唯一公开的队列深度）。
    pub out_queue_len: u64,
}

impl Counters {
    /// 计数器是否「倒退」了。
    ///
    /// 累计值只增不减；一旦变小，只可能是网卡重插、驱动重载或计数器清零。
    /// 此时直接相减会得到天文数字（`u64` 下溢或巨大的 `saturating_sub` 差值），
    /// 界面上就是「瞬间 18 EB/s」这种荒唐读数 —— 必须先识别再归零。
    pub fn regressed_from(&self, previous: &Counters) -> bool {
        self.in_octets < previous.in_octets || self.out_octets < previous.out_octets
    }
}

/// 一次采样得到的单个适配器状态（全部为只读数据，没有任何推断字段）。
#[derive(Debug, Clone)]
pub struct RawAdapter {
    /// 稳定标识：LUID（跨重启、跨索引变化不变）。
    pub id: String,
    /// 接口索引（同一台机器内有效，重插后可能变化）。
    pub index: u32,
    pub luid: u64,
    /// 系统给这块网卡起的名字（界面上熟悉的「以太网 2」「WLAN」）。
    pub name: String,
    /// 驱动描述（「Intel(R) Wi-Fi 6 AX200 160MHz」这类）。
    pub description: String,
    pub class: AdapterClass,
    /// 介质标签（以太网 / Wi-Fi / 移动宽带……）。
    pub media: String,
    /// MAC 地址（无物理地址时为空串）。
    pub mac: String,
    pub mtu: u32,
    /// `IF_OPER_STATUS` 原值。
    pub oper_status: u32,
    /// `NET_IF_ADMIN_STATUS` 原值（1 = 已启用）。
    pub admin_status: u32,
    /// `NET_IF_MEDIA_CONNECT_STATE` 原值。
    pub connect_state: u32,
    /// 是否硬件接口（驱动声明的物理网卡标志位）。
    pub hardware: bool,
    /// 是否插着连接器（无线网卡为假）。
    pub connector_present: bool,
    /// 驱动上报的「链路未连接」标志位。
    pub not_media_connected: bool,
    /// 协商发送速率（bit/s，0 = 未知 / 未连接）。
    pub transmit_speed_bps: u64,
    /// 协商接收速率（bit/s）。
    pub receive_speed_bps: u64,
    pub counters: Counters,
}

impl RawAdapter {
    /// 链路是否处于「已连接且可用」——三个信号都看，缺一不可。
    ///
    /// 只看 `OperStatus` 会在网线拔出的瞬间骗人：某些驱动在拔线后仍报告
    /// `IF_OPER_UP` 几十毫秒，而 `MediaConnectState` 已经变成未连接。反过来只看
    /// 连接状态，则无线网卡在「已关联但未认证」时会显示成正常。
    pub fn link_up(&self) -> bool {
        self.oper_status == IF_OPER_UP
            && self.connect_state != MEDIA_CONNECT_DISCONNECTED
            && !self.not_media_connected
    }

    /// 协商速率：取收发方向的较大值（半双工网卡两个方向相同）。
    pub fn link_speed_bps(&self) -> u64 {
        self.transmit_speed_bps.max(self.receive_speed_bps)
    }
}

/// 采集层错误。**不带事件码**：事件码的唯一登记处是 `loadloom_core::codes`，
/// 采集层只负责如实描述故障，由上层决定用哪个码记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NicError {
    pub kind: NicErrorKind,
    /// 面向人的细节（含 Win32 状态码时写明数值，便于检索）。
    pub detail: String,
}

/// 采集层错误类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicErrorKind {
    /// 当前平台没有实现。
    Unsupported,
    /// 系统调用返回错误。
    CallFailed,
    /// 系统调用成功但返回的数据不合法。
    Malformed,
}

impl NicError {
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self {
            kind: NicErrorKind::Unsupported,
            detail: detail.into(),
        }
    }

    pub fn call_failed(status: u32) -> Self {
        Self {
            kind: NicErrorKind::CallFailed,
            detail: format!("GetIfTable2 返回 Win32 状态 {status}"),
        }
    }

    pub fn malformed(detail: impl Into<String>) -> Self {
        Self {
            kind: NicErrorKind::Malformed,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for NicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for NicError {}

// ---------------------------------------------------------------------------
// 对外接口
// ---------------------------------------------------------------------------

/// 当前平台是否支持读取网卡计数器。
pub fn is_supported() -> bool {
    cfg!(windows)
}

/// 平台标签（写进日志与报告，避免用户拿 Linux 版报告问 Windows 的问题）。
pub fn platform_label() -> &'static str {
    std::env::consts::OS
}

/// 枚举本机全部接口的当前状态与累计计数器。
///
/// 返回**全部**接口（含虚拟 / 隧道），由调用方按 [`AdapterClass`] 过滤 ——
/// 过滤规则与展示策略属于业务决策，采集层不做取舍，否则「为什么看不到我的网卡」
/// 这类问题会变成无法解释的黑箱。
pub fn list_adapters() -> Result<Vec<RawAdapter>, NicError> {
    backend::list_adapters()
}

// ---------------------------------------------------------------------------
// 纯函数：分类与标签（跨平台可测，不依赖任何系统调用）
// ---------------------------------------------------------------------------

/// 分类所需的原始事实。单独成结构体是为了让单元测试能构造出与真机一致的组合。
#[derive(Debug, Clone, Copy)]
pub struct AdapterFacts<'a> {
    pub alias: &'a str,
    pub description: &'a str,
    pub if_type: u32,
    pub media_type: u32,
    /// `TunnelType`，0 表示非隧道。
    pub tunnel_type: u32,
    /// 驱动声明的硬件接口标志。
    pub hardware: bool,
    /// 点对点端点接口（PPP / VPN 端点）。
    pub endpoint: bool,
}

/// 虚拟 / 隧道接口的**名称特征**（全部小写匹配）。
///
/// 只按标志位过滤是不够的：Hyper-V 虚拟网卡与某些 USB 网卡在标志上完全一样
/// （`HardwareInterface = 1`），只有名字能区分。反过来只按名字过滤也不够：伪装成
/// 真实厂商名的第三方虚拟适配器（ZeroTier、Tailscale 的 Wintun 设备）名字里
/// 往往带自家产品名，但硬件标志为假。两条判据同时使用。
const VIRTUAL_NAME_PATTERNS: &[&str] = &[
    // Hyper-V / 容器 / WSL
    "hyper-v",
    "vethernet",
    "wsl",
    "docker",
    "container",
    // 桌面虚拟化
    "vmware",
    "virtualbox",
    "virtual adapter",
    "virtual ethernet",
    "virtio",
    "parallels",
    "qemu",
    // 环回 / 抓包 / 测试用适配器
    "loopback adapter",
    "npcap",
    "winpcap",
    "microsoft km-test",
    // 隧道 / 拨号 / 无线直连
    "wan miniport",
    "teredo",
    "isatap",
    "6to4",
    "wi-fi direct",
    "wifi direct",
    "wireless host",
    "bluetooth device (personal area network)",
    "pseudo-interface",
    // 第三方 VPN / 组网客户端（WinTun 类虚拟网卡）
    "wintun",
    "wireguard",
    "tailscale",
    "zerotier",
    "openvpn",
    "softether",
    "anyconnect",
    "forticlient",
    "fortinet",
    "checkpoint",
    "sonicwall",
    "pulse secure",
    "juniper",
    "nordlynx",
    "mullvad",
    "protonvpn",
    "hamachi",
    "radmin",
    "tap-windows",
];

/// 判定适配器类别。
pub fn classify(facts: &AdapterFacts<'_>) -> AdapterClass {
    let alias = facts.alias.to_ascii_lowercase();
    let description = facts.description.to_ascii_lowercase();
    let named = |pattern: &str| alias.contains(pattern) || description.contains(pattern);

    // 1) 回环优先：它同时也是「虚拟」的，但类别要更精确。
    if facts.if_type == IF_TYPE_SOFTWARE_LOOPBACK
        || facts.media_type == MEDIA_LOOPBACK
        || named("loopback")
    {
        return AdapterClass::Loopback;
    }
    // 2) 隧道：驱动明确标注了隧道类型，或接口类型就是隧道。
    if facts.tunnel_type != 0 || facts.if_type == IF_TYPE_TUNNEL || facts.media_type == MEDIA_TUNNEL
    {
        return AdapterClass::Tunnel;
    }
    // 3) 端点接口（PPP / VPN 端点）：既不是物理网卡，也不该混进隧道统计。
    if facts.endpoint {
        return AdapterClass::Other;
    }
    // 4) 已知虚拟设备的名称特征。
    if VIRTUAL_NAME_PATTERNS.iter().any(|pattern| named(pattern)) {
        return AdapterClass::Virtual;
    }
    // 5) 驱动没声明硬件接口 —— 不是物理网卡。
    if !facts.hardware {
        return AdapterClass::Virtual;
    }
    AdapterClass::Physical
}

/// 介质标签：优先看 IANA `ifType`（它区分以太网 / Wi-Fi / 移动宽带），
/// 缺失时退回 `NDIS_MEDIUM`。
pub fn media_label(if_type: u32, media_type: u32) -> String {
    let label = match if_type {
        IF_TYPE_ETHERNET_CSMACD => "以太网",
        IF_TYPE_IEEE80211 => "Wi-Fi",
        IF_TYPE_IEEE80216_WMAN => "WiMAX",
        IF_TYPE_WWANPP => "移动宽带",
        IF_TYPE_SOFTWARE_LOOPBACK => "回环",
        IF_TYPE_TUNNEL => "隧道",
        _ => match media_type {
            MEDIA_802_3 => "以太网",
            MEDIA_NATIVE_802_11 | MEDIA_WIRELESS_WAN => "无线",
            MEDIA_TUNNEL => "隧道",
            MEDIA_LOOPBACK => "回环",
            other => return format!("媒体类型 {other}"),
        },
    };
    label.to_owned()
}

/// `IF_OPER_STATUS` 的中文标签（日志与报告直接引用，避免两处各译一遍）。
pub fn oper_status_label(status: u32) -> &'static str {
    match status {
        IF_OPER_UP => "已启用",
        IF_OPER_DOWN => "已断开",
        IF_OPER_TESTING => "自检中",
        IF_OPER_DORMANT => "休眠",
        IF_OPER_NOT_PRESENT => "设备不存在",
        IF_OPER_LOWER_LAYER_DOWN => "下层断开",
        IF_OPER_UNKNOWN => "未知",
        _ => "未知状态",
    }
}

/// `NET_IF_MEDIA_CONNECT_STATE` 的中文标签。
pub fn connect_state_label(state: u32) -> &'static str {
    match state {
        MEDIA_CONNECT_CONNECTED => "已连接",
        MEDIA_CONNECT_DISCONNECTED => "未连接",
        _ => "连接状态未知",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(alias: &'a str, description: &'a str) -> AdapterFacts<'a> {
        AdapterFacts {
            alias,
            description,
            if_type: IF_TYPE_ETHERNET_CSMACD,
            media_type: MEDIA_802_3,
            tunnel_type: 0,
            hardware: true,
            endpoint: false,
        }
    }

    #[test]
    fn real_ethernet_and_wifi_are_physical() {
        assert_eq!(
            classify(&facts("以太网", "Intel(R) Ethernet Controller I225-V")),
            AdapterClass::Physical
        );
        let mut wifi = facts("WLAN", "Intel(R) Wi-Fi 6 AX200 160MHz");
        wifi.if_type = IF_TYPE_IEEE80211;
        wifi.media_type = MEDIA_NATIVE_802_11;
        assert_eq!(classify(&wifi), AdapterClass::Physical);
    }

    #[test]
    fn hyper_v_and_vendor_virtual_adapters_are_virtual() {
        assert_eq!(
            classify(&facts(
                "vEthernet (Default Switch)",
                "Hyper-V Virtual Ethernet Adapter"
            )),
            AdapterClass::Virtual
        );
        assert_eq!(
            classify(&facts(
                "以太网 2",
                "VMware Virtual Ethernet Adapter for VMnet8"
            )),
            AdapterClass::Virtual
        );
        let mut wireguard = facts("wg0", "WireGuard Tunnel");
        wireguard.hardware = false;
        assert_eq!(classify(&wireguard), AdapterClass::Virtual);
    }

    #[test]
    fn hardware_flag_decides_when_the_name_looks_real() {
        // 名字像真网卡，但驱动没声明硬件接口 —— 不能当成物理网卡统计。
        let mut fake = facts("Ethernet 5", "Some Virtual NIC");
        fake.hardware = false;
        assert_eq!(classify(&fake), AdapterClass::Virtual);
    }

    #[test]
    fn loopback_tunnel_and_endpoint_get_their_own_classes() {
        let mut loopback = facts(
            "Loopback Pseudo-Interface 1",
            "Software Loopback Interface 1",
        );
        loopback.if_type = IF_TYPE_SOFTWARE_LOOPBACK;
        loopback.media_type = MEDIA_LOOPBACK;
        assert_eq!(classify(&loopback), AdapterClass::Loopback);

        let mut tunnel = facts("本地连接* 1", "Microsoft Teredo Tunneling Adapter");
        tunnel.tunnel_type = 12;
        assert_eq!(classify(&tunnel), AdapterClass::Tunnel);

        let mut endpoint = facts("宽带连接", "WAN Miniport (PPPOE)");
        endpoint.endpoint = true;
        assert_eq!(classify(&endpoint), AdapterClass::Other);
    }

    #[test]
    fn counter_regression_is_detected() {
        let previous = Counters {
            in_octets: 1_000,
            out_octets: 2_000,
            ..Counters::default()
        };
        assert!(!previous.regressed_from(&previous));
        let bigger = Counters {
            in_octets: 1_500,
            out_octets: 2_500,
            ..Counters::default()
        };
        assert!(!bigger.regressed_from(&previous));
        let reset = Counters {
            in_octets: 0,
            out_octets: 10,
            ..Counters::default()
        };
        assert!(reset.regressed_from(&previous));
    }

    #[test]
    fn link_up_requires_all_three_signals() {
        let mut adapter = RawAdapter {
            id: "luid-1".to_owned(),
            index: 1,
            luid: 1,
            name: "以太网".to_owned(),
            description: "test".to_owned(),
            class: AdapterClass::Physical,
            media: "以太网".to_owned(),
            mac: "AA-BB".to_owned(),
            mtu: 1500,
            oper_status: IF_OPER_UP,
            admin_status: 1,
            connect_state: MEDIA_CONNECT_CONNECTED,
            hardware: true,
            connector_present: true,
            not_media_connected: false,
            transmit_speed_bps: 1_000_000_000,
            receive_speed_bps: 1_000_000_000,
            counters: Counters::default(),
        };
        assert!(adapter.link_up());
        adapter.not_media_connected = true;
        assert!(!adapter.link_up(), "驱动标志说未连接就不能算在线");
        adapter.not_media_connected = false;
        adapter.connect_state = MEDIA_CONNECT_DISCONNECTED;
        assert!(!adapter.link_up());
        assert_eq!(adapter.link_speed_bps(), 1_000_000_000);
    }

    #[test]
    fn labels_never_claim_a_status_we_cannot_see() {
        assert_eq!(oper_status_label(IF_OPER_DORMANT), "休眠");
        assert_eq!(oper_status_label(999), "未知状态");
        assert_eq!(connect_state_label(MEDIA_CONNECT_UNKNOWN), "连接状态未知");
        assert_eq!(media_label(IF_TYPE_IEEE80211, MEDIA_NATIVE_802_11), "Wi-Fi");
        assert_eq!(media_label(0, 42), "媒体类型 42");
        assert!(AdapterClass::Physical.monitored_by_default());
        assert!(!AdapterClass::Virtual.monitored_by_default());
    }
}
