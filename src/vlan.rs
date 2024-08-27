use crate::{Node, Nodes, BROADCAST_INTERVAL, MAX_SILENT_INTERVALS};
use if_addrs::get_if_addrs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, watch};
use tokio::time::sleep;
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

const BROADCAST_PORT: u16 = 12345;

pub async fn discover() -> Result<
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

    let own_ip = get_own_private_ip().unwrap_or(Ipv4Addr::new(127, 0, 0, 1));
    info!("Own IP address: {}", own_ip);

    let socket = Arc::new(
        UdpSocket::bind(("0.0.0.0", BROADCAST_PORT))
            .await
            .expect("Failed to bind socket"),
    );
    socket.set_broadcast(true).expect("Failed to set broadcast");

    let ip_str = own_ip.to_string();
    let octets: Vec<&str> = ip_str.split('.').collect();

    if octets.len() != 4 {
        return Err("Invalid IP address format".into());
    }

    let broadcast_ip = format!("{}.{}.{}.255", octets[0], octets[1], octets[2]);

    let _ = up_tx.send(());

    let nodes_clone = Arc::clone(&nodes);
    let socket_clone = Arc::clone(&socket);
    let mut shutdown_clone = shutdown_rx.clone();
    // Task for broadcasting
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_clone.changed() => {
                    info!("Shutdown signal received, stopping broadcast task");
                    break;
                }
                _ = sleep(BROADCAST_INTERVAL) => {
                    nodes_clone.reap();
                    match socket_clone
                        .send_to(&own_ip.octets(), (broadcast_ip.as_str(), BROADCAST_PORT))
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

    // Task for receiving
    tokio::spawn(async move {
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
                                    if !nodes_clone.test(discovered_ip) {
                                        info!("Discovered new node: {}", discovered_ip);
                                    }
                                    // always add nodes to refresh last_seen
                                    nodes_clone.add(discovered_ip, None, None);
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

    Ok((up_rx, fin_rx, shutdown_tx, Arc::clone(&nodes)))
}

pub fn get_ip(interface: &str) -> Option<Ipv4Addr> {
    let addrs = match get_if_addrs() {
        Ok(addrs) => addrs,
        Err(e) => {
            warn!("Failed to get network interfaces: {}", e);
            return None;
        }
    };

    for addr in addrs {
        if addr.name == interface {
            if let IpAddr::V4(ip) = addr.ip() {
                return Some(ip);
            }
        }
    }

    None
}

pub fn get_own_private_ip() -> Option<Ipv4Addr> {
    let addrs = match get_if_addrs() {
        Ok(addrs) => addrs,
        Err(e) => {
            warn!("Failed to get network interfaces: {}", e);
            return None;
        }
    };

    for addr in addrs {
        if let IpAddr::V4(ip) = addr.ip() {
            if ip.is_private() && ip.octets()[0] == 10 {
                return Some(ip);
            }
        }
    }

    None
}

fn extract_private_ip(addr: &SocketAddr) -> Option<Ipv4Addr> {
    match addr.ip() {
        IpAddr::V4(ipv4) => {
            if ipv4.is_private() && ipv4.octets()[0] == 10 {
                Some(ipv4)
            } else {
                None
            }
        }
        IpAddr::V6(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use std::thread::sleep;

    #[test]
    fn test_get_own_private_ip() {
        let ip: Option<Ipv4Addr> = get_own_private_ip();
        assert_eq!(ip, None);
    }

    #[test]
    fn test_nodes_add_and_test() {
        let nodes: Nodes = Nodes::new();
        nodes.add(Ipv4Addr::from_str("127.0.0.1").unwrap());
        assert!(nodes.test(Ipv4Addr::from_str("127.0.0.1").unwrap()));
        assert!(!nodes.test(Ipv4Addr::from_str("192.168.0.1").unwrap()));
    }

    #[test]
    fn test_nodes_all() {
        let nodes: Nodes = Nodes::new();
        nodes.add(Ipv4Addr::from_str("127.0.0.1").unwrap());
        nodes.add(Ipv4Addr::from_str("192.168.0.1").unwrap());
        let all_nodes: Vec<Node> = nodes.all();
        assert_eq!(all_nodes.len(), 2);
        assert!(all_nodes
            .iter()
            .any(|node| node.ip == Ipv4Addr::from_str("127.0.0.1").unwrap()));
        assert!(all_nodes
            .iter()
            .any(|node| node.ip == Ipv4Addr::from_str("192.168.0.1").unwrap()));
    }

    #[test]
    fn test_nodes_reap() {
        let nodes: Nodes = Nodes::new();
        nodes.add(Ipv4Addr::from_str("127.0.0.1").unwrap());
        nodes.add(Ipv4Addr::from_str("192.168.0.1").unwrap());
        sleep(Duration::from_secs(
            (MAX_SILENT_INTERVALS + 1) * BROADCAST_INTERVAL.as_secs(),
        ));
        nodes.reap();
        assert_eq!(nodes.all().len(), 0);
    }
}
