use crate::{Nodes, BROADCAST_INTERVAL};
use if_addrs::{get_if_addrs, IfAddr};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, watch};
use tokio::time::sleep;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PrivateBroadcastInterface {
    ip: Ipv4Addr,
    broadcast: Ipv4Addr,
}

pub async fn discover(
    broadcast_port: u16,
) -> Result<
    (
        oneshot::Receiver<()>,
        oneshot::Receiver<()>,
        watch::Sender<()>,
        Arc<Nodes>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let nodes = Arc::new(Nodes::new());

    let (shutdown_tx, mut shutdown_rx) = watch::channel(());
    let (up_tx, up_rx) = oneshot::channel();
    let (fin_tx, fin_rx) = oneshot::channel();

    let interface =
        private_broadcast_interface().ok_or("no private IPv4 broadcast interface found")?;
    let own_ip = interface.ip;
    let broadcast_ip = interface.broadcast;
    info!("Own IP address: {}", own_ip);

    let socket = Arc::new(UdpSocket::bind(("0.0.0.0", broadcast_port)).await?);
    socket.set_broadcast(true)?;

    let nodes_clone = Arc::clone(&nodes);
    let socket_clone = Arc::clone(&socket);
    let mut broadcast_shutdown_rx = shutdown_rx.clone();
    let broadcast_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = broadcast_shutdown_rx.changed() => {
                    info!("Shutdown signal received, stopping broadcast task");
                    break;
                }
                _ = sleep(BROADCAST_INTERVAL) => {
                    nodes_clone.reap();
                    match socket_clone
                        .send_to(&own_ip.octets(), (broadcast_ip, broadcast_port))
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Failed to send broadcast: {}", e);
                        }
                    }
                }
            }
        }
    });

    let nodes_clone = Arc::clone(&nodes);

    let receive_task = tokio::spawn(async move {
        let mut buffer = [0; 1024];
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Shutdown signal received, stopping receive task");
                    break;
                }
                result = socket.recv_from(&mut buffer) => {
                    match result {
                        Ok((_, src_addr)) => {
                            if let Some(discovered_ip) = extract_private_ip(&src_addr) {
                                if discovered_ip != own_ip {
                                    if !nodes_clone.test(&discovered_ip) {
                                        info!("Discovered new node: {}", discovered_ip);
                                    }
                                    // always add nodes to refresh last_seen
                                    let is_self = own_ip == discovered_ip;
                                    nodes_clone.add(discovered_ip, None, None, is_self);
                                };
                            } else {
                                warn!("Received broadcast from non-private IP: {}", src_addr.ip());
                            }
                        }
                        Err(e) => {
                            warn!("Error receiving broadcast: {}", e);
                        }
                    }
                }
            }
        }
    });

    tokio::spawn(async move {
        let _ = broadcast_task.await;
        let _ = receive_task.await;
        let _ = fin_tx.send(());
    });

    let _ = up_tx.send(());

    Ok((up_rx, fin_rx, shutdown_tx, Arc::clone(&nodes)))
}

pub fn get_own_private_ip() -> Option<Ipv4Addr> {
    private_broadcast_interface().map(|interface| interface.ip)
}

fn private_broadcast_interface() -> Option<PrivateBroadcastInterface> {
    let addrs = match get_if_addrs() {
        Ok(addrs) => addrs,
        Err(e) => {
            warn!("Failed to get network interfaces: {}", e);
            return None;
        }
    };

    for addr in addrs {
        if let IfAddr::V4(v4) = addr.addr {
            if is_discoverable_private_ip(v4.ip) {
                let broadcast = v4
                    .broadcast
                    .unwrap_or_else(|| ipv4_broadcast(v4.ip, v4.netmask));
                return Some(PrivateBroadcastInterface {
                    ip: v4.ip,
                    broadcast,
                });
            }
        }
    }

    None
}

fn extract_private_ip(addr: &SocketAddr) -> Option<Ipv4Addr> {
    match addr.ip() {
        IpAddr::V4(ipv4) => {
            if is_discoverable_private_ip(ipv4) {
                Some(ipv4)
            } else {
                None
            }
        }
        IpAddr::V6(_) => None,
    }
}

fn is_discoverable_private_ip(ip: Ipv4Addr) -> bool {
    ip.is_private() && !ip.is_loopback()
}

fn ipv4_broadcast(ip: Ipv4Addr, netmask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(ip) | !u32::from(netmask))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_own_private_ip_returns_private_address_when_available() {
        if let Some(ip) = get_own_private_ip() {
            assert!(is_discoverable_private_ip(ip));
        }
    }

    #[test]
    fn extract_private_ip_accepts_rfc1918_sources() {
        let cases = [
            Ipv4Addr::new(10, 0, 0, 10),
            Ipv4Addr::new(172, 16, 0, 10),
            Ipv4Addr::new(192, 168, 0, 10),
        ];

        for ip in cases {
            let addr = SocketAddr::new(IpAddr::V4(ip), 12345);
            assert_eq!(extract_private_ip(&addr), Some(ip));
        }
    }

    #[test]
    fn extract_private_ip_rejects_loopback_public_and_ipv6_sources() {
        let cases = [
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 12345),
            "[::1]:12345".parse().unwrap(),
        ];

        for addr in cases {
            assert_eq!(extract_private_ip(&addr), None);
        }
    }

    #[test]
    fn ipv4_broadcast_uses_netmask() {
        assert_eq!(
            ipv4_broadcast(Ipv4Addr::new(10, 1, 2, 3), Ipv4Addr::new(255, 255, 0, 0)),
            Ipv4Addr::new(10, 1, 255, 255)
        );
        assert_eq!(
            ipv4_broadcast(
                Ipv4Addr::new(192, 168, 1, 20),
                Ipv4Addr::new(255, 255, 255, 0)
            ),
            Ipv4Addr::new(192, 168, 1, 255)
        );
    }
}
