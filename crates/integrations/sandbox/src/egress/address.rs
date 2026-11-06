use std::{
    error::Error,
    fmt,
    net::{IpAddr, Ipv4Addr},
    str::FromStr,
};

#[must_use]
pub const fn is_public(address: IpAddr) -> bool {
    match address.to_canonical() {
        IpAddr::V4(v4) => {
            !(v4.octets()[0] == 0
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast())
        }
        IpAddr::V6(v6) => {
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast())
        }
    }
}

pub const SPECIAL: [Cidr; 2] = [
    Cidr {
        network: IpAddr::V4(Ipv4Addr::new(100, 64, 0, 0)),
        bits: 10,
    },
    Cidr {
        network: IpAddr::V4(Ipv4Addr::new(192, 0, 0, 0)),
        bits: 24,
    },
];

#[must_use]
pub fn reachable(address: IpAddr, deny: &[Cidr]) -> bool {
    is_public(address) && !deny.iter().any(|cidr| cidr.contains(address))
}

#[must_use]
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    pattern.strip_prefix("*.").map_or_else(
        || host == pattern,
        |domain| {
            host.strip_suffix(domain)
                .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
        },
    )
}

#[must_use]
pub fn valid_pattern(host: &str) -> bool {
    let bare = host.strip_prefix("*.").unwrap_or(host);
    !bare.is_empty()
        && bare.len() <= 253
        && bare
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    bits: u8,
}

impl Cidr {
    #[must_use]
    pub fn contains(&self, address: IpAddr) -> bool {
        match (address.to_canonical(), self.network) {
            (IpAddr::V4(address), IpAddr::V4(network)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.bits)).unwrap_or(0);
                u32::from(address) & mask == u32::from(network) & mask
            }
            (IpAddr::V6(address), IpAddr::V6(network)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.bits))
                    .unwrap_or(0);
                u128::from(address) & mask == u128::from(network) & mask
            }
            _ => false,
        }
    }
}

impl FromStr for Cidr {
    type Err = NotCidr;

    fn from_str(text: &str) -> Result<Self, NotCidr> {
        let not = |cause: Option<Box<dyn Error + Send + Sync>>| NotCidr {
            text: text.to_owned(),
            cause,
        };
        let (network, bits) = text.split_once('/').unwrap_or((text, ""));
        let network: IpAddr = network
            .parse()
            .map_err(|error| not(Some(Box::new(error))))?;
        let most = if network.is_ipv4() { 32 } else { 128 };
        let bits = if bits.is_empty() {
            most
        } else {
            bits.parse().map_err(|error| not(Some(Box::new(error))))?
        };
        if bits > most {
            return Err(not(None));
        }
        Ok(Self { network, bits })
    }
}

#[derive(Debug)]
pub struct NotCidr {
    text: String,
    cause: Option<Box<dyn Error + Send + Sync>>,
}

impl fmt::Display for NotCidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is not a CIDR", self.text)
    }
}

impl Error for NotCidr {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn Error + 'static))
    }
}
