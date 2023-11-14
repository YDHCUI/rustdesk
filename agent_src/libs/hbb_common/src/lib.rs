pub mod compress;
#[path = "./protos/message.rs"]
pub mod message_proto;
#[path = "./protos/rendezvous.rs"]
pub mod rendezvous_proto;
pub use bytes;
pub use futures;
pub use protobuf;
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs},
    time::{self, SystemTime, UNIX_EPOCH},
};
pub use tokio;
pub use tokio_util;
pub mod tcp;
pub mod udp;
pub mod bytes_codec;
#[cfg(feature = "quic")]
pub mod quic;
pub use anyhow::{self, bail};
pub use futures_util;
pub mod config;
pub mod fs;
pub use sodiumoxide;

#[cfg(feature = "quic")]
pub type Stream = quic::Connection;
#[cfg(not(feature = "quic"))]
pub type Stream = tcp::FramedStream;

#[inline]
pub async fn sleep(sec: f32) {
    tokio::time::sleep(time::Duration::from_secs_f32(sec)).await;
}

#[macro_export]
macro_rules! allow_err {
    ($e:expr) => {
        if let Err(err) = $e {
            println!("{:?}",err);
        } else {
        }
    };
}

#[inline]
pub fn timeout<T: std::future::Future>(ms: u64, future: T) -> tokio::time::Timeout<T> {
    tokio::time::timeout(std::time::Duration::from_millis(ms), future)
}

pub type ResultType<F, E = anyhow::Error> = anyhow::Result<F, E>;

/// Certain router and firewalls scan the packet and if they
/// find an IP address belonging to their pool that they use to do the NAT mapping/translation, so here we mangle the ip address

pub struct AddrMangle();

impl AddrMangle {
    pub fn encode(addr: SocketAddr) -> Vec<u8> {
        match addr {
            SocketAddr::V4(addr_v4) => {
                let tm = (SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_micros() as u32) as u128;
                let ip = u32::from_le_bytes(addr_v4.ip().octets()) as u128;
                let port = addr.port() as u128;
                let v = ((ip + tm) << 49) | (tm << 17) | (port + (tm & 0xFFFF));
                let bytes = v.to_le_bytes();
                let mut n_padding = 0;
                for i in bytes.iter().rev() {
                    if i == &0u8 {
                        n_padding += 1;
                    } else {
                        break;
                    }
                }
                bytes[..(16 - n_padding)].to_vec()
            }
            _ => {
                panic!("Only support ipv4");
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> SocketAddr {
        let mut padded = [0u8; 16];
        padded[..bytes.len()].copy_from_slice(&bytes);
        let number = u128::from_le_bytes(padded);
        let tm = (number >> 17) & (u32::max_value() as u128);
        let ip = (((number >> 49) - tm) as u32).to_le_bytes();
        let port = (number & 0xFFFFFF) - (tm & 0xFFFF);
        SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]),
            port as u16,
        ))
    }
}


pub fn to_socket_addr(host: &str) -> ResultType<SocketAddr> {
    let addrs: Vec<SocketAddr> = host.to_socket_addrs()?.collect();
    if addrs.is_empty() {
        bail!("Failed to solve {}", host);
    }
    Ok(addrs[0])
}
