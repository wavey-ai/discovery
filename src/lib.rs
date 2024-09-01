pub mod dns;
pub mod server;
pub mod vlan;

use if_addrs::get_if_addrs;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tokio::time::{Duration, Instant};
use tracing::{error, info, warn};

const DNS_CHECK_INTERVAL: Duration = Duration::from_secs(3600);
const BROADCAST_INTERVAL: Duration = Duration::from_secs(5);
const MAX_SILENT_INTERVALS: u64 = 10;

#[derive(Debug, Clone)]
pub struct Node {
    ip: Ipv4Addr,
    tag: Option<String>,
    seq: Option<u32>,
    last_seen: Instant,
}

impl Node {
    pub fn ip(&self) -> Ipv4Addr {
        self.ip.clone()
    }
    pub fn tag(&self) -> Option<&String> {
        self.tag.as_ref()
    }
    pub fn seq(&self) -> Option<u32> {
        self.seq
    }
}

pub struct Nodes {
    own_ips: HashSet<Ipv4Addr>,
    data: Arc<RwLock<HashMap<Ipv4Addr, Node>>>,
    tx: broadcast::Sender<Ipv4Addr>,
}

impl Nodes {
    pub fn new(interfaces: Vec<&str>) -> Self {
        let (tx, _) = broadcast::channel::<Ipv4Addr>(16);
        let mut own_ips = HashSet::new();
        for interface in interfaces {
            if let Some(ip) = get_ip(interface) {
                own_ips.insert(ip);
            }
        }
        own_ips.insert(Ipv4Addr::new(127, 0, 0, 1));
        Nodes {
            data: Arc::new(RwLock::new(HashMap::new())),
            tx,
            own_ips,
        }
    }

    pub fn rx(&self) -> broadcast::Receiver<Ipv4Addr> {
        self.tx.subscribe()
    }

    pub fn test(&self, ip: Ipv4Addr) -> bool {
        let lock = self.data.read().unwrap();
        lock.contains_key(&ip)
    }

    pub fn add(&self, ip: Ipv4Addr, tag: Option<String>, seq: Option<u32>) -> bool {
        let mut lock = self.data.write().unwrap();
        if !lock.contains_key(&ip) && !self.own_ips.contains(&ip) {
            lock.insert(
                ip.clone(),
                Node {
                    ip,
                    last_seen: Instant::now(),
                    tag,
                    seq,
                },
            );

            let _ = self.tx.send(ip);

            return true;
        }

        false
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
