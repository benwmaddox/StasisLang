//! Portable LAN address selection and platform permission/discovery seams.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use thiserror::Error;

/// A deterministic, secret-free failure to choose a native LAN address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LanAddressError {
    #[error("native LAN interface enumeration failed")]
    EnumerationFailed,
    #[error("no usable native LAN IPv4 address is available")]
    NoUsableAddress,
    #[error("multiple native LAN IPv4 addresses are available; set an explicit override")]
    MultipleUsableAddresses,
}

/// Chooses the sole usable address after deterministic filtering and deduplication.
///
/// Interface names and ordering are deliberately ignored. If multiple distinct
/// addresses remain, callers must obtain an explicit choice from the user.
pub fn select_lan_ipv4(
    candidates: impl IntoIterator<Item = Ipv4Addr>,
) -> Result<Ipv4Addr, LanAddressError> {
    let candidates = candidates
        .into_iter()
        .filter(|ip| is_usable_lan_ipv4(*ip))
        .collect::<BTreeSet<_>>();
    match candidates.len() {
        0 => Err(LanAddressError::NoUsableAddress),
        1 => Ok(*candidates.first().expect("one candidate")),
        _ => Err(LanAddressError::MultipleUsableAddresses),
    }
}

/// Enumerates native interfaces and chooses an address without external probes.
pub fn native_lan_ipv4() -> Result<Ipv4Addr, LanAddressError> {
    native_lan_ipv4_with(|| {
        if_addrs::get_if_addrs().map(|interfaces| {
            interfaces
                .into_iter()
                .filter_map(|interface| match interface.addr {
                    if_addrs::IfAddr::V4(address) => Some(address.ip),
                    if_addrs::IfAddr::V6(_) => None,
                })
        })
    })
}

fn native_lan_ipv4_with<I, E>(
    enumerate: impl FnOnce() -> Result<I, E>,
) -> Result<Ipv4Addr, LanAddressError>
where
    I: IntoIterator<Item = Ipv4Addr>,
{
    let candidates = enumerate().map_err(|_| LanAddressError::EnumerationFailed)?;
    select_lan_ipv4(candidates)
}

fn is_usable_lan_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, _, _] = ip.octets();
    first != 0
        && first != 127
        && first < 224
        && !ip.is_link_local()
        && !(first == 192 && second == 0 && ip.octets()[2] == 0)
        && !ip.is_documentation()
        && !(first == 198 && (second == 18 || second == 19))
}

/// Current platform status for permission to communicate on the local network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalNetworkPermission {
    NotRequired,
    Unknown,
    Granted,
    Denied,
    Restricted,
}

/// Current status of the optional platform discovery mechanism (Bonjour, NSD, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryStatus {
    Unsupported,
    Idle,
    Starting,
    Active,
    PermissionDenied,
    Failed,
}

/// A platform discovery registration. Pairing secrets must never be included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryService<'a> {
    pub instance_name: &'a str,
    pub service_type: &'a str,
    pub port: u16,
}

/// Platform-owned permission and discovery operations.
///
/// Implementations live in platform shells. The network crate does not emulate
/// discovery when the operating system service is unavailable.
pub trait LanPlatformShell {
    fn local_network_permission(&self) -> LocalNetworkPermission;
    fn discovery_status(&self) -> DiscoveryStatus;
    fn request_local_network_permission(&mut self) -> Result<(), LanShellError>;
    fn start_discovery(&mut self, service: DiscoveryService<'_>) -> Result<(), LanShellError>;
    fn stop_discovery(&mut self) -> Result<(), LanShellError>;
}

/// Secret-free platform failure categories; never carry URLs or OS error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LanShellError {
    #[error("platform LAN operation is unsupported")]
    Unsupported,
    #[error("local network permission was denied")]
    PermissionDenied,
    #[error("platform LAN operation failed")]
    Failed,
}

/// Default for hosts without a permission/discovery integration.
/// Manual socket access can still be attempted; availability is not permission.
#[derive(Debug, Default)]
pub struct UnsupportedLanShell;

impl LanPlatformShell for UnsupportedLanShell {
    fn local_network_permission(&self) -> LocalNetworkPermission {
        LocalNetworkPermission::Unknown
    }

    fn discovery_status(&self) -> DiscoveryStatus {
        DiscoveryStatus::Unsupported
    }

    fn request_local_network_permission(&mut self) -> Result<(), LanShellError> {
        Err(LanShellError::Unsupported)
    }

    fn start_discovery(&mut self, _service: DiscoveryService<'_>) -> Result<(), LanShellError> {
        Err(LanShellError::Unsupported)
    }

    fn stop_discovery(&mut self) -> Result<(), LanShellError> {
        Ok(())
    }
}

/// Manual invite links remain available independently of discovery support/status.
pub fn manual_invites_available(permission: LocalNetworkPermission) -> bool {
    !matches!(
        permission,
        LocalNetworkPermission::Denied | LocalNetworkPermission::Restricted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_filters_deduplicates_and_is_order_independent() {
        let expected = Ipv4Addr::new(192, 168, 4, 20);
        let forward = [
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::UNSPECIFIED,
            expected,
            expected,
            Ipv4Addr::BROADCAST,
            Ipv4Addr::new(224, 0, 0, 1),
        ];
        assert_eq!(select_lan_ipv4(forward), Ok(expected));
        assert_eq!(select_lan_ipv4(forward.into_iter().rev()), Ok(expected));
    }

    #[test]
    fn offline_private_addresses_are_usable() {
        assert_eq!(
            select_lan_ipv4([Ipv4Addr::new(10, 40, 0, 7)]),
            Ok(Ipv4Addr::new(10, 40, 0, 7))
        );
        assert_eq!(
            select_lan_ipv4([Ipv4Addr::new(169, 254, 8, 9)]),
            Err(LanAddressError::NoUsableAddress)
        );
    }

    #[test]
    fn zero_and_multiple_addresses_fail_without_guessing() {
        assert_eq!(
            select_lan_ipv4([Ipv4Addr::LOCALHOST, Ipv4Addr::UNSPECIFIED]),
            Err(LanAddressError::NoUsableAddress)
        );
        assert_eq!(
            select_lan_ipv4([Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(192, 168, 0, 2),]),
            Err(LanAddressError::MultipleUsableAddresses)
        );
    }

    #[test]
    fn reserved_candidates_are_not_automatic_invites() {
        for ip in [
            "0.1.2.3",
            "240.1.2.3",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "198.18.0.1",
            "192.0.0.1",
        ] {
            assert_eq!(
                select_lan_ipv4([ip.parse().unwrap()]),
                Err(LanAddressError::NoUsableAddress)
            );
        }
    }

    #[test]
    fn native_adapter_returns_only_policy_valid_results() {
        match native_lan_ipv4() {
            Ok(ip) => assert_eq!(select_lan_ipv4([ip]), Ok(ip)),
            Err(LanAddressError::NoUsableAddress | LanAddressError::MultipleUsableAddresses) => {}
            Err(error) => panic!("native adapter failed: {error}"),
        }
    }

    #[test]
    fn enumeration_failure_is_typed_and_secret_free() {
        let error = native_lan_ipv4_with(|| Err::<[Ipv4Addr; 0], _>("secret-value"))
            .expect_err("enumeration must fail");
        assert_eq!(error, LanAddressError::EnumerationFailed);
        assert!(!format!("{error:?} {error}").contains("secret-value"));
    }

    #[test]
    fn unsupported_shell_does_not_fake_permission_or_discovery() {
        let mut shell = UnsupportedLanShell;
        assert_eq!(
            shell.local_network_permission(),
            LocalNetworkPermission::Unknown
        );
        assert_eq!(shell.discovery_status(), DiscoveryStatus::Unsupported);
        assert!(manual_invites_available(shell.local_network_permission()));
        assert_eq!(
            shell.request_local_network_permission(),
            Err(LanShellError::Unsupported)
        );
        assert_eq!(
            shell.start_discovery(DiscoveryService {
                instance_name: "Stasis",
                service_type: "_http._tcp",
                port: 8080,
            }),
            Err(LanShellError::Unsupported)
        );
        assert_eq!(shell.stop_discovery(), Ok(()));
        assert_eq!(shell.stop_discovery(), Ok(()));
        assert_eq!(shell.discovery_status(), DiscoveryStatus::Unsupported);
        assert!(manual_invites_available(shell.local_network_permission()));
    }

    #[test]
    fn manual_invites_do_not_depend_on_discovery() {
        for permission in [
            LocalNetworkPermission::NotRequired,
            LocalNetworkPermission::Unknown,
            LocalNetworkPermission::Granted,
        ] {
            assert!(manual_invites_available(permission));
        }
        assert!(!manual_invites_available(LocalNetworkPermission::Denied));
        assert!(!manual_invites_available(
            LocalNetworkPermission::Restricted
        ));
    }
}
