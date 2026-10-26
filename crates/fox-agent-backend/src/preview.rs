use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub pid: u32,
    pub session_key: String,
    pub port: u16,
    pub url: String,
}

#[cfg(windows)]
pub fn listeners() -> Result<Vec<(u32, u16)>, String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;
    let mut size = 0u32;
    unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if size < 4 || size > 16 * 1024 * 1024 {
        return Err("无法读取本机监听端口".into());
    }
    // u64 storage gives the Windows table its required alignment.
    let mut buffer = vec![0u64; (size as usize + 7) / 8];
    let result = unsafe {
        GetExtendedTcpTable(
            buffer.as_mut_ptr().cast(),
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if result != 0 {
        return Err(format!("读取监听端口失败：{result}"));
    }
    let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
    let count = unsafe { (*table).dwNumEntries } as usize;
    let offset = std::mem::offset_of!(MIB_TCPTABLE_OWNER_PID, table);
    if offset
        + count
            * std::mem::size_of::<
                windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPROW_OWNER_PID,
            >()
        > size as usize
    {
        return Err("监听端口列表不完整".into());
    }
    let rows = unsafe {
        std::slice::from_raw_parts(
            std::ptr::addr_of!((*table).table)
                .cast::<windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPROW_OWNER_PID>(),
            count,
        )
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            let address = std::net::Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes());
            (address.is_loopback() || address.is_unspecified())
                .then_some((row.dwOwningPid, u16::from_be(row.dwLocalPort as u16)))
        })
        .collect())
}

#[cfg(not(windows))]
pub fn listeners() -> Result<Vec<(u32, u16)>, String> {
    Err("当前平台尚未提供预览进程核验".into())
}
