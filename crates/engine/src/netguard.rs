use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NetguardError {
    #[error("adresse interne refusée: {0}")]
    Denied(IpAddr),
    #[error("entrée invalide dans la liste d'autorisation: {0}")]
    InvalidEntry(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cidr {
    network: IpAddr,
    prefix: u8,
}

impl Cidr {
    fn parse(text: &str) -> Result<Self, NetguardError> {
        let invalid = || NetguardError::InvalidEntry(text.to_owned());
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => {
                (address, Some(prefix.parse::<u8>().map_err(|_| invalid())?))
            }
            None => (text, None),
        };
        let network: IpAddr = address.parse().map_err(|_| invalid())?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        if prefix > max {
            return Err(invalid());
        }
        Ok(Self { network, prefix })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(network) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(network) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Policy {
    allowed: Vec<Cidr>,
}

impl Policy {
    pub fn parse_allowed(list: &str) -> Result<Self, NetguardError> {
        let allowed = list
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(Cidr::parse)
            .collect::<Result<_, _>>()?;
        Ok(Self { allowed })
    }

    pub fn check_ip(&self, ip: IpAddr) -> Result<(), NetguardError> {
        let ip = normalize(ip);
        if self.allowed.iter().any(|cidr| cidr.contains(ip)) || !is_internal(ip) {
            Ok(())
        } else {
            Err(NetguardError::Denied(ip))
        }
    }

    // Un nom d'hôte est contrôlé à la connexion par le résolveur ; seule une IP littérale l'est ici.
    pub fn check_host(&self, host: &str) -> Result<(), NetguardError> {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        match bare.parse::<IpAddr>() {
            Ok(ip) => self.check_ip(ip),
            Err(_) => Ok(()),
        }
    }
}

fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_internal_v4(v4),
        IpAddr::V6(v6) => is_internal_v6(v6),
    }
}

fn is_internal_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
}

fn is_internal_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || first & 0xfe00 == 0xfc00
        || first & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn denied(policy: &Policy, text: &str) -> bool {
        policy.check_ip(ip(text)).is_err()
    }

    #[test]
    fn public_addresses_are_allowed() {
        let policy = Policy::default();
        for text in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
        ] {
            assert!(!denied(&policy, text), "{text} devrait passer");
        }
    }

    #[test]
    fn internal_ipv4_ranges_are_denied_by_default() {
        let policy = Policy::default();
        for text in [
            "127.0.0.1",
            "127.255.255.254",
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(denied(&policy, text), "{text} devrait être refusée");
        }
    }

    #[test]
    fn adjacent_public_ranges_are_not_over_blocked() {
        let policy = Policy::default();
        for text in [
            "172.15.255.255",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "11.0.0.1",
        ] {
            assert!(!denied(&policy, text), "{text} devrait passer");
        }
    }

    #[test]
    fn internal_ipv6_ranges_are_denied() {
        let policy = Policy::default();
        for text in ["::1", "::", "fc00::1", "fd12:3456::1", "fe80::1", "ff02::1"] {
            assert!(denied(&policy, text), "{text} devrait être refusée");
        }
    }

    #[test]
    fn ipv4_mapped_ipv6_is_judged_as_ipv4() {
        let policy = Policy::default();
        assert!(denied(&policy, "::ffff:127.0.0.1"));
        assert!(denied(&policy, "::ffff:169.254.169.254"));
        assert!(!denied(&policy, "::ffff:8.8.8.8"));
    }

    #[test]
    fn explicit_allow_list_opens_only_what_is_named() {
        let policy = Policy::parse_allowed("192.168.1.0/24, 10.0.0.5").unwrap();
        assert!(!denied(&policy, "192.168.1.77"));
        assert!(!denied(&policy, "10.0.0.5"));
        assert!(denied(&policy, "192.168.2.1"));
        assert!(denied(&policy, "10.0.0.6"));
        assert!(denied(&policy, "127.0.0.1"));
    }

    #[test]
    fn allow_list_supports_ipv6_and_loopback_ranges() {
        let policy = Policy::parse_allowed("127.0.0.0/8,fd00::/8").unwrap();
        assert!(!denied(&policy, "127.0.0.1"));
        assert!(!denied(&policy, "fd00::42"));
        assert!(denied(&policy, "fe80::1"));
    }

    #[test]
    fn invalid_allow_list_entries_are_rejected() {
        for text in ["nope", "10.0.0.0/33", "10.0.0.0/x", "::1/129", "1.2.3"] {
            assert!(
                Policy::parse_allowed(text).is_err(),
                "{text} devrait être refusée"
            );
        }
        assert!(
            Policy::parse_allowed("")
                .unwrap()
                .check_ip(ip("127.0.0.1"))
                .is_err()
        );
    }

    #[test]
    fn literal_ip_hosts_are_checked_and_names_are_left_to_the_resolver() {
        let policy = Policy::default();
        assert!(policy.check_host("127.0.0.1").is_err());
        assert!(policy.check_host("[::1]").is_err());
        assert!(policy.check_host("169.254.169.254").is_err());
        assert!(policy.check_host("example.com").is_ok());
        assert!(policy.check_host("8.8.8.8").is_ok());
    }
}
