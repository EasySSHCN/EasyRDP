//! device_info.rs — 匿名遥测设备信息采集（R-20261119-004：对齐 EasySSH 统计字段）
//!
//! 对齐 EasySSH src-tauri/src/stats.rs 的 8 项设备信息上报口径：
//!   软件版本 / 操作系统 / CPU / 内存(档位) / 磁盘容量(档位) / 显卡 / 系统语言代码 / 地域(服务端生成)
//!
//! 采集方式：纯 WinAPI 原生采集（注册表直读 + 系统调用，全程无子进程、无脚本执行，
//! 移植自 EasySSH feedback.rs winffi 同族实现）；任一项失败自动跳过，不含任何个人身份信息。

/* ---------------- 档位（与 EasySSH stats.rs 完全一致的口径） ---------------- */

/// 内存归档位：8GB 以下 / 8-16 / 16-32 / 32GB 以上
pub fn mem_bucket(gb_str: &str) -> String {
    let gb: f64 = gb_str.trim_end_matches("GB").trim().parse().unwrap_or(0.0);
    if gb <= 0.0 {
        return String::new();
    }
    if gb < 8.0 {
        "8GB以下".into()
    } else if gb < 16.0 {
        "8-16GB".into()
    } else if gb < 32.0 {
        "16-32GB".into()
    } else {
        "32GB以上".into()
    }
}

/// 磁盘容量归档位（取第一块固定磁盘总容量）
pub fn disk_bucket(total_gb: u64) -> String {
    if total_gb == 0 {
        return String::new();
    }
    match total_gb {
        0..=256 => "≤256GB".into(),
        257..=512 => "256-512GB".into(),
        513..=1024 => "512GB-1TB".into(),
        1025..=2048 => "1-2TB".into(),
        _ => "2TB以上".into(),
    }
}

/* ---------------- Windows 原生采集 ---------------- */

#[cfg(windows)]
pub(crate) mod winffi {
    pub fn to_wide_null(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    pub fn from_wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    pub const RRF_RT_REG_SZ: u32 = 0x2;
    pub const HKEY_LOCAL_MACHINE: isize = 0x8000_0002;

    #[link(name = "advapi32")]
    extern "system" {
        pub fn RegGetValueW(
            hkey: isize,
            lpsubkey: *const u16,
            lpvalue: *const u16,
            dwflags: u32,
            pdwtype: *mut u32,
            pvdata: *mut u8,
            pcbdata: *mut u32,
        ) -> i32;
    }

    /// 读注册表字符串值（RegGetValueW 自动开合句柄）
    pub fn reg_sz(subkey: &str, value: &str) -> Option<String> {
        let sk = to_wide_null(subkey);
        let vn = to_wide_null(value);
        let mut buf = [0u8; 1024];
        let mut size = buf.len() as u32;
        let mut ty = 0u32;
        let rc = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, sk.as_ptr(), vn.as_ptr(), RRF_RT_REG_SZ, &mut ty, buf.as_mut_ptr(), &mut size)
        };
        if rc != 0 {
            return None;
        }
        let u16s: Vec<u16> = buf[..size as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let s = from_wide(&u16s);
        let s = s.trim_end_matches('\0').trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GlobalMemoryStatusEx(lpbuffer: *mut MEMORYSTATUSEX) -> i32;
        pub fn GetLogicalDriveStringsW(nbufferlength: u32, lpbuffer: *mut u16) -> u32;
        pub fn GetDriveTypeW(lprootpathname: *const u16) -> u32;
        pub fn GetDiskFreeSpaceExW(
            lpdirectoryname: *const u16,
            lpfreebytesavailabletocaller: *mut u64,
            lptotalnumberofbytes: *mut u64,
            lptotalnumberoffreebytes: *mut u64,
        ) -> i32;
    }

    #[repr(C)]
    pub struct MEMORYSTATUSEX {
        pub dw_length: u32,
        pub dw_memory_load: u32,
        pub ull_total_phys: u64,
        pub ull_avail_phys: u64,
        pub ull_total_pagefile: u64,
        pub ull_avail_pagefile: u64,
        pub ull_total_virtual: u64,
        pub ull_avail_virtual: u64,
        pub ull_avail_extended_virtual: u64,
    }

    pub fn total_mem_gb() -> Option<String> {
        let mut ms = MEMORYSTATUSEX {
            dw_length: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            dw_memory_load: 0, ull_total_phys: 0, ull_avail_phys: 0,
            ull_total_pagefile: 0, ull_avail_pagefile: 0,
            ull_total_virtual: 0, ull_avail_virtual: 0, ull_avail_extended_virtual: 0,
        };
        if unsafe { GlobalMemoryStatusEx(&mut ms) } != 0 {
            Some(format!("{:.1}GB", ms.ull_total_phys as f64 / 1073741824.0))
        } else {
            None
        }
    }

    /// 第一块固定磁盘的总容量（GB，十进制口径与 EasySSH winffi 一致）
    pub fn first_fixed_disk_gb() -> Option<u64> {
        let mut buf = [0u16; 512];
        let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) };
        if n == 0 || n as usize > buf.len() {
            return None;
        }
        for root in from_wide(&buf[..n as usize]).split('\0').filter(|s| !s.is_empty()) {
            let ty = unsafe { GetDriveTypeW(to_wide_null(root).as_ptr()) };
            if ty != 3 {
                continue; // 3 = DRIVE_FIXED
            }
            let root_w = to_wide_null(root);
            let mut free = 0u64;
            let mut total = 0u64;
            let mut dummy = 0u64;
            if unsafe { GetDiskFreeSpaceExW(root_w.as_ptr(), &mut free, &mut total, &mut dummy) } != 0 && total > 0 {
                return Some(total / 1_000_000_000);
            }
        }
        None
    }
}

/// 上报字段值封顶（对齐服务端 ST_MAX_VALUE 语义，防止超长）
fn capped(v: String) -> serde_json::Value {
    serde_json::json!(v.chars().take(160).collect::<String>())
}

fn put_if_present(fields: &mut serde_json::Map<String, serde_json::Value>, k: &str, v: String) {
    let v = v.trim().to_string();
    if !v.is_empty() {
        fields.insert(k.to_string(), capped(v));
    }
}

/// 采集设备信息并注入上报字段（Windows：注册表 + 系统 API；非 Windows：跳过各项）
#[cfg(windows)]
pub fn enrich_fields(fields: &mut serde_json::Map<String, serde_json::Value>) {
    // 操作系统：产品名 + DisplayVersion（不含 Build 明细，降低指纹粒度）
    let product = winffi::reg_sz(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName")
        .unwrap_or_else(|| "Windows".into());
    let display =
        winffi::reg_sz(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "DisplayVersion").unwrap_or_default();
    put_if_present(fields, "操作系统", if display.is_empty() { product } else { format!("{product} {display}") });
    // CPU 型号截断到家族级别（去掉频率后缀），降低指纹粒度
    if let Some(n) =
        winffi::reg_sz(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
    {
        let fam = n.split('@').next().unwrap_or(&n).trim().to_string();
        put_if_present(fields, "CPU", fam);
    }
    if let Some(gb) = winffi::total_mem_gb() {
        put_if_present(fields, "内存", mem_bucket(&gb));
    }
    if let Some(total_gb) = winffi::first_fixed_disk_gb() {
        put_if_present(fields, "磁盘容量", disk_bucket(total_gb));
    }
    // 显卡只传首块（显示适配器类注册表枚举，与 EasySSH 同口径）
    for i in 0..10 {
        if let Some(desc) = winffi::reg_sz(
            &format!(
                r"SYSTEM\CurrentControlSet\Control\Class\{{4d36e968-e325-11ce-bfc1-08002be10318}}\{:04}",
                i
            ),
            "DriverDesc",
        ) {
            put_if_present(fields, "显卡", desc);
            break;
        }
    }
}

#[cfg(not(windows))]
pub fn enrich_fields(_fields: &mut serde_json::Map<String, serde_json::Value>) {}

/* ---------------- 单测 ---------------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_buckets_match_easyssh() {
        assert_eq!(mem_bucket("7.9GB"), "8GB以下");
        assert_eq!(mem_bucket("8.0GB"), "8-16GB");
        assert_eq!(mem_bucket("15.9GB"), "8-16GB");
        assert_eq!(mem_bucket("16.0GB"), "16-32GB");
        assert_eq!(mem_bucket("32.0GB"), "32GB以上");
        assert_eq!(mem_bucket(""), "");
    }

    #[test]
    fn disk_buckets_match_easyssh() {
        assert_eq!(disk_bucket(0), "");
        assert_eq!(disk_bucket(256), "≤256GB");
        assert_eq!(disk_bucket(476), "256-512GB");
        assert_eq!(disk_bucket(953), "512GB-1TB");
        assert_eq!(disk_bucket(1863), "1-2TB");
        assert_eq!(disk_bucket(3726), "2TB以上");
    }
}
