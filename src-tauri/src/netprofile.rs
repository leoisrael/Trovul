// Trovul - Windows network profile (Public / Private / Domain) of the LAN
// adapter. Windows Firewall blocks inbound traffic for most apps on Public
// networks, which hides this PC from the other one; the UI explains how to
// switch the network to Private.
//
// Only the adapter that holds this PC's LAN address counts: virtual adapters
// (Hyper-V, WSL, VPNs) often sit on "Unidentified" public networks and would
// raise false alarms.

use crate::com;
use serde::Serialize;
use std::net::Ipv4Addr;
use windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::Networking::NetworkListManager::{
    INetworkListManager, NetworkListManager, NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED,
    NLM_NETWORK_CATEGORY_PUBLIC,
};
use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Public,
    Private,
    Domain,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkProfile {
    /// The name Windows shows for the network (Wi-Fi name, "Network 2", …).
    pub name: String,
    pub category: Category,
    /// Wi-Fi rather than Ethernet: picks the right Settings page.
    pub wifi: bool,
}

/// Profile of the network that holds `ip` (this PC's LAN address).
pub fn profile_for(ip: Ipv4Addr) -> Result<NetworkProfile, String> {
    let adapter = adapter_for(ip).ok_or_else(|| format!("No network adapter has {ip}"))?;
    com::ensure_initialized();
    unsafe {
        let nlm: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)
            .map_err(|e| format!("Network List Manager: {e}"))?;
        // Connections carry the adapter id; the adapter's own NetworkGuid is
        // not an id the Network List Manager knows.
        let connections = nlm.GetNetworkConnections().map_err(|e| e.to_string())?;
        let network = loop {
            let mut item = [None];
            let mut fetched = 0u32;
            connections
                .Next(&mut item, Some(&mut fetched))
                .map_err(|e| e.to_string())?;
            let Some(connection) = item[0].take() else {
                return Err(format!("No network connection uses adapter {}", adapter.id));
            };
            let id = connection.GetAdapterId().map_err(|e| e.to_string())?;
            if format!("{id:?}").eq_ignore_ascii_case(&adapter.id) {
                break connection.GetNetwork().map_err(|e| e.to_string())?;
            }
        };
        let category = match network.GetCategory().map_err(|e| e.to_string())? {
            NLM_NETWORK_CATEGORY_PUBLIC => Category::Public,
            NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED => Category::Domain,
            _ => Category::Private,
        };
        let name = network.GetName().map(|n| n.to_string()).unwrap_or_default();
        Ok(NetworkProfile {
            name,
            category,
            wifi: adapter.wifi,
        })
    }
}

struct Adapter {
    /// Adapter GUID without braces, as `GUID`'s Debug prints it.
    id: String,
    wifi: bool,
}

fn adapter_for(ip: Ipv4Addr) -> Option<Adapter> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    // The list can grow between calls; ERROR_BUFFER_OVERFLOW updates `size`.
    for _ in 0..3 {
        // u64 elements keep the buffer aligned for the structs inside it.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let result =
            unsafe { GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(first), &mut size) };
        match result {
            0 => return unsafe { find_adapter(first, ip) },
            e if e == ERROR_BUFFER_OVERFLOW.0 => continue,
            e => {
                log::debug!("GetAdaptersAddresses failed: {e}");
                return None;
            }
        }
    }
    None
}

/// Walk the adapter list filled by `GetAdaptersAddresses`.
unsafe fn find_adapter(
    mut adapter: *const IP_ADAPTER_ADDRESSES_LH,
    ip: Ipv4Addr,
) -> Option<Adapter> {
    while let Some(a) = adapter.as_ref() {
        let mut unicast = a.FirstUnicastAddress;
        while let Some(u) = unicast.as_ref() {
            let sockaddr = u.Address.lpSockaddr;
            if !sockaddr.is_null() && (*sockaddr).sa_family == AF_INET {
                let b = (*sockaddr.cast::<SOCKADDR_IN>()).sin_addr.S_un.S_un_b;
                if Ipv4Addr::new(b.s_b1, b.s_b2, b.s_b3, b.s_b4) == ip {
                    let name = a.AdapterName.to_string().ok()?;
                    return Some(Adapter {
                        id: name.trim_matches(['{', '}']).to_string(),
                        wifi: a.IfType == IF_TYPE_IEEE80211,
                    });
                }
            }
            unicast = u.Next;
        }
        adapter = a.Next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the real profile of this PC's LAN adapter (needs a network):
    /// `cargo test -- --ignored network_profile --nocapture`.
    #[test]
    #[ignore]
    fn network_profile_of_this_pc() {
        let std::net::IpAddr::V4(ip) = crate::network::local_ip().expect("no LAN address") else {
            panic!("LAN address is IPv6");
        };
        let profile = profile_for(ip).unwrap();
        println!("{ip}: {profile:?}");
        assert!(!profile.name.is_empty());
    }
}
