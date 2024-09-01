pub mod dns;
pub mod server;
pub mod vlan;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
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
    pub fn addr(&self, port: u16) -> SocketAddr {
        SocketAddr::new(std::net::IpAddr::V4(self.ip()), port)
    }
    pub fn tag(&self) -> Option<&String> {
        self.tag.as_ref()
    }
    pub fn seq(&self) -> Option<u32> {
        self.seq
    }
}

pub struct Nodes {
    data: Arc<RwLock<HashMap<Ipv4Addr, Node>>>,
    tx: broadcast::Sender<Ipv4Addr>,
}

impl Nodes {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel::<Ipv4Addr>(16);
        Nodes {
            data: Arc::new(RwLock::new(HashMap::new())),
            tx,
        }
    }

    pub fn rx(&self) -> broadcast::Receiver<Ipv4Addr> {
        self.tx.subscribe()
    }

    pub fn test(&self, ip: &Ipv4Addr) -> bool {
        let lock = self.data.read().unwrap();
        lock.contains_key(ip)
    }

    pub fn add(&self, ip: Ipv4Addr, tag: Option<String>, seq: Option<u32>) {
        let mut lock = self.data.write().unwrap();
        // only notify if the ip was initially absent
        if !lock.contains_key(&ip) {
            let _ = self.tx.send(ip);
        }
        // always overwrite to update last seen
        lock.insert(
            ip.clone(),
            Node {
                ip,
                last_seen: Instant::now(),
                tag,
                seq,
            },
        );
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
