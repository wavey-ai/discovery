use if_addrs::get_if_addrs;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::time::{sleep, Duration, Instant};
use tracing::{debug, error, info, warn};

const BROADCAST_INTERVAL: Duration = Duration::from_secs(5);
const BROADCAST_PORT: u16 = 12345;
const MAX_SILENT_INTERVALS: u64 = 10;

#[derive(Debug, Clone)]
pub struct Node {
    pub ip: Ipv4Addr,
    last_seen: Instant,
}

pub struct Nodes {
    data: Arc<RwLock<HashMap<Ipv4Addr, Node>>>,
}

impl Nodes {
    pub fn new() -> Self {
        Nodes {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn test(&self, ip: Ipv4Addr) -> bool {
        let lock = self.data.read().unwrap();
        lock.contains_key(&ip)
    }

    pub fn add(&self, ip: Ipv4Addr) {
        let mut lock = self.data.write().unwrap();
        lock.entry(ip.clone()).or_insert(Node {
            ip,
            last_seen: Instant::now(),
        });
    }

    pub fn all(&self) -> Vec<Node> {
        let lock = self.data.read().unwrap();
        lock.values().cloned().collect()
    }

    fn reap(&self) {
        let mut nodes_map = self.data.write().unwrap();
        let current_time = Instant::now();
        nodes_map.retain(|_, node| {
            let node_last_seen_duration = current_time.duration_since(node.last_seen);
            let silent_intervals_seconds = MAX_SILENT_INTERVALS * BROADCAST_INTERVAL.as_secs();
            node_last_seen_duration.as_secs() <= silent_intervals_seconds
        });
    }
}

pub async fn discover(nodes: Arc<Nodes>) -> oneshot::Sender<()> {
    let own_ip = get_own_private_ip().unwrap_or(Ipv4Addr::new(127, 0, 0, 1));
    info!("Own IP address: {}", own_ip);

    let socket = Arc::new(
        UdpSocket::bind(("0.0.0.0", BROADCAST_PORT))
            .await
            .expect("Failed to bind socket"),
    );
    socket.set_broadcast(true).expect("Failed to set broadcast");

    let broadcast_ip = format!(
        "{}.255",
        own_ip.to_string().rsplit('.').nth(1).unwrap_or("")
    );

    let (broadcast_tx, broadcast_rx) = oneshot::channel();
    let (receive_tx, receive_rx) = oneshot::channel();
    let (reap_tx, reap_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();

    tokio::spawn(broadcast_own_ip(
        own_ip.clone(),
        socket.clone(),
        broadcast_ip,
        broadcast_rx,
    ));
    tokio::spawn(receive_broadcasts(
        socket.clone(),
        own_ip.clone(),
        nodes.clone(),
        receive_rx,
    ));
    tokio::spawn(reap_nodes(nodes.clone(), reap_rx));

    tokio::spawn(async move {
        cancel_rx.await;
        broadcast_tx.send(());
        receive_tx.send(());
        reap_tx.send(());
    });

    cancel_tx
}

async fn receive_broadcasts(
    socket: Arc<UdpSocket>,
    own_ip: Ipv4Addr,
    nodes: Arc<Nodes>,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    let mut buffer = [0; 1024];
    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                info!("Cancelling receive_broadcasts task");
                break;
            }
            result = socket.recv_from(&mut buffer) => {
                match result {
                    Ok((_, src_addr)) => {
                        if let Some(discovered_ip) = extract_private_ip(&src_addr) {
                            if discovered_ip != own_ip {
                              if !nodes.test(discovered_ip) {
                                  info!("Discovered new node: {}", discovered_ip);
                              }
                              // always add nodes to refresh last_seen
                              nodes.add(discovered_ip);
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
}

async fn broadcast_own_ip(
    own_ip: Ipv4Addr,
    socket: Arc<UdpSocket>,
    broadcast_ip: String,
    mut cancel_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                info!("Cancelling broadcast_own_ip task");
                break;
            }
            _ = sleep(BROADCAST_INTERVAL) => {
                match socket
                    .send_to(&own_ip.octets(), (broadcast_ip.as_str(), BROADCAST_PORT))
                    .await
                {
                    Ok(_) => {
                    }
                    Err(e) => {
                        warn!("Failed to send broadcast: {}", e);
                    }
                }
            }
        }
    }
}

async fn reap_nodes(nodes: Arc<Nodes>, mut cancel_rx: oneshot::Receiver<()>) {
    loop {
        tokio::select! {
            _ = &mut cancel_rx => {
                info!("Cancelling reap_nodes task");
                break;
            }
            _ = sleep(BROADCAST_INTERVAL) => {
                nodes.reap();
            }
        }
    }
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
