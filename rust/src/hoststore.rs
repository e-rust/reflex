//! Acesso ao estado do host (usuários, config, convite) na pasta da máquina
//! ([`paths::machine_dir`]), compartilhado pelo agente e pelo serviço.

use crate::auth::store::{FileStorage, SignedFile};
use crate::platform::secret::Scope;
use crate::platform::paths;
use crate::proto::msg::MachineStatus;

use crate::identity_store::{load_or_create_identity, load_or_create_secret};

/// O nome desta máquina.
pub fn hostname() -> String {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname").ok().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    name.unwrap_or_else(|| "Reflex".into())
}

/// Store assinado do host.
pub fn open_store() -> Result<SignedFile<FileStorage>, String> {
    let dir = paths::machine_dir();
    let secret = load_or_create_secret(&dir.join("secret.bin"))?;
    Ok(SignedFile::new(FileStorage { path: dir.join("store.json") }, secret))
}

/// Impressão digital do certificado da máquina (o ID a conferir no viewer).
pub fn machine_fingerprint() -> Result<String, String> {
    let dir = paths::machine_dir();
    let id = load_or_create_identity(&dir.join("identity.bin"), &hostname(), Scope::Machine)?;
    Ok(id.access_id().to_string())
}

/// IPs locais (best-effort, via resolução do próprio nome), formatados pra
/// digitar no viewer: IPv4 primeiro, IPv6 entre colchetes.
pub fn local_addresses(port: u16) -> Vec<String> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<std::net::SocketAddr> = (hostname().as_str(), port).to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    usable_addresses(addrs)
}

/// Filtra e ordena: sem loopback nem link-local (169.254.x, fe80::), que não
/// servem pra conectar de outra máquina. Pura.
pub fn usable_addresses(mut addrs: Vec<std::net::SocketAddr>) -> Vec<String> {
    use std::net::IpAddr;
    addrs.retain(|a| match a.ip() {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local(),
        IpAddr::V6(v6) => !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80,
    });
    addrs.sort_by_key(|a| (a.is_ipv6(), a.ip()));
    addrs.dedup();
    // `SocketAddr` já põe o IPv6 entre colchetes.
    addrs.iter().map(|a| a.to_string()).collect()
}

/// Monta o [`MachineStatus`] pra aba "Esta máquina".
pub fn status(now: u64) -> Result<MachineStatus, String> {
    let store = open_store()?;
    let data = store.load().map_err(|e| e.to_string())?;
    let port = if data.config.port == 0 { crate::proto::DEFAULT_PORT } else { data.config.port };
    let invite = data.invite.current_code(now).map(|code| crate::proto::msg::InviteInfo { code, level: data.invite.level, next_rotation: data.invite.next_rotation(now) });
    Ok(MachineStatus {
        fingerprint: machine_fingerprint().unwrap_or_default(),
        machine_name: hostname(),
        port,
        has_master: data.has_master(),
        user_count: data.users.len() as u32,
        invite,
        addresses: local_addresses(port),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_filtered_and_ipv6_bracketed() {
        let input = vec![
            "[fe80::1]:47800".parse().unwrap(),
            "[2804:d56::1]:47800".parse().unwrap(),
            "169.254.83.107:47800".parse().unwrap(),
            "192.168.100.8:47800".parse().unwrap(),
            "127.0.0.1:47800".parse().unwrap(),
        ];
        assert_eq!(usable_addresses(input), vec!["192.168.100.8:47800", "[2804:d56::1]:47800"]);
    }
}
