//! Lógica pura da interface (testável sem janela).

use crate::proto::DEFAULT_PORT;

/// Normaliza o que o usuário digitou pra `ip:porta` (ou `nome:porta`),
/// pondo a porta padrão quando falta. `None` se vazio.
pub fn normalize_address(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    // IPv6 entre colchetes: [::1]:47800 ou [::1].
    if let Some(rest) = t.strip_prefix('[') {
        let (addr, tail) = rest.split_once(']')?;
        let port = tail.strip_prefix(':').and_then(|p| p.parse::<u16>().ok()).unwrap_or(DEFAULT_PORT);
        return Some(format!("[{addr}]:{port}"));
    }
    // host:porta (um único ':').
    if let Some((host, port)) = t.rsplit_once(':')
        && !host.is_empty()
        && port.parse::<u16>().is_ok()
    {
        return Some(t.to_string());
    }
    Some(format!("{t}:{DEFAULT_PORT}"))
}

/// `ip:porta` ou `nome:porta` → `SocketAddr`. Nome é resolvido no DNS na
/// hora (bloqueia: chame fora da thread da janela). Um nome não muda quando
/// o IP muda — a VM no Default Switch do Hyper-V troca de IP a cada reinício
/// do PC, mas `<máquina>.mshome.net` continua achando.
pub fn resolve(address: &str) -> Result<std::net::SocketAddr, String> {
    if let Ok(ip) = address.parse() {
        return Ok(ip);
    }
    use std::net::ToSocketAddrs;
    let found = address.to_socket_addrs().map_err(|e| format!("não achei {address}: {e}"))?;
    prefer_ipv4(found).ok_or_else(|| format!("{address} não tem endereço"))
}

/// O 1º IPv4, senão o 1º que houver: na LAN o caminho testado é IPv4, e o
/// IPv6 de link-local que alguns nomes devolvem primeiro costuma não servir. Pura.
pub fn prefer_ipv4(addrs: impl IntoIterator<Item = std::net::SocketAddr>) -> Option<std::net::SocketAddr> {
    let all: Vec<_> = addrs.into_iter().collect();
    all.iter().find(|a| a.is_ipv4()).or(all.first()).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_default_port() {
        assert_eq!(normalize_address("192.168.0.5"), Some("192.168.0.5:47800".into()));
        assert_eq!(normalize_address("  host.lan "), Some("host.lan:47800".into()));
    }

    #[test]
    fn keeps_given_port() {
        assert_eq!(normalize_address("10.0.0.1:9000"), Some("10.0.0.1:9000".into()));
    }

    #[test]
    fn ipv6_in_brackets() {
        assert_eq!(normalize_address("[::1]"), Some("[::1]:47800".into()));
        assert_eq!(normalize_address("[fe80::1]:5000"), Some("[fe80::1]:5000".into()));
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(normalize_address("   "), None);
    }

    #[test]
    fn ip_addresses_need_no_lookup() {
        assert_eq!(resolve("127.0.0.1:47800").unwrap().port(), 47800);
        assert!(resolve("[::1]:47800").unwrap().is_ipv6());
    }

    #[test]
    fn names_are_resolved() {
        let a = resolve("localhost:47800").unwrap();
        assert!(a.ip().is_loopback());
        assert_eq!(a.port(), 47800);
        assert!(resolve("nao-existe.invalid:47800").is_err());
    }

    #[test]
    fn ipv4_is_preferred() {
        let v6: std::net::SocketAddr = "[fe80::1]:1".parse().unwrap();
        let v4: std::net::SocketAddr = "10.0.0.2:1".parse().unwrap();
        assert_eq!(prefer_ipv4([v6, v4]), Some(v4));
        assert_eq!(prefer_ipv4([v6]), Some(v6));
        assert_eq!(prefer_ipv4([]), None);
    }
}
