pub mod dns;
pub mod vlan;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tokio::time::{Duration, Instant};

const DNS_CHECK_INTERVAL: Duration = Duration::from_secs(3600);
const BROADCAST_INTERVAL: Duration = Duration::from_secs(5);
const MAX_SILENT_INTERVALS: u64 = 10;

#[derive(Debug, Clone)]
pub struct Node {
    ip: Ipv4Addr,
    tag: Option<String>,
    seq: Option<u32>,
    last_seen: Instant,
    is_self: bool,
}

impl Node {
    pub fn ip(&self) -> Ipv4Addr {
        self.ip
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
    pub fn is_self(&self) -> bool {
        self.is_self
    }
}

pub struct Nodes {
    data: Arc<RwLock<HashMap<Ipv4Addr, Node>>>,
    tx: broadcast::Sender<Node>,
}

impl Nodes {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel::<Node>(16);
        Nodes {
            data: Arc::new(RwLock::new(HashMap::new())),
            tx,
        }
    }

    pub fn rx(&self) -> broadcast::Receiver<Node> {
        self.tx.subscribe()
    }

    pub fn test(&self, ip: &Ipv4Addr) -> bool {
        let lock = self.data.read().unwrap();
        lock.contains_key(ip)
    }

    pub fn add(&self, ip: Ipv4Addr, tag: Option<String>, seq: Option<u32>, is_self: bool) {
        let node = Node {
            ip,
            last_seen: Instant::now(),
            tag,
            seq,
            is_self,
        };

        let mut lock = self.data.write().unwrap();
        // only notify if the ip was initially absent
        if !lock.contains_key(&ip) {
            let _ = self.tx.send(node.clone());
        }
        // always overwrite to update last seen
        lock.insert(ip.clone(), node);
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

impl Default for Nodes {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn nodes_add_and_test() {
        let nodes = Nodes::new();
        let local = Ipv4Addr::new(127, 0, 0, 1);
        let remote = Ipv4Addr::new(192, 168, 0, 1);

        nodes.add(local, None, None, true);

        assert!(nodes.test(&local));
        assert!(!nodes.test(&remote));
    }

    #[test]
    fn nodes_all_returns_added_nodes() {
        let nodes = Nodes::new();
        let first = Ipv4Addr::new(127, 0, 0, 1);
        let second = Ipv4Addr::new(192, 168, 0, 1);

        nodes.add(first, Some("local".into()), Some(1), true);
        nodes.add(second, Some("remote".into()), Some(2), false);

        let all_nodes = nodes.all();
        assert_eq!(all_nodes.len(), 2);
        assert!(all_nodes.iter().any(|node| node.ip() == first));
        assert!(all_nodes.iter().any(|node| node.ip() == second));
    }

    #[test]
    fn nodes_notify_only_on_first_add() {
        let nodes = Nodes::new();
        let mut rx = nodes.rx();
        let ip = Ipv4Addr::new(192, 168, 0, 1);

        nodes.add(ip, Some("first".into()), Some(1), false);
        let node = rx.try_recv().unwrap();
        assert_eq!(node.ip(), ip);
        assert_eq!(node.tag().map(String::as_str), Some("first"));

        nodes.add(ip, Some("refresh".into()), Some(2), false);
        assert!(rx.try_recv().is_err());

        let refreshed = nodes
            .all()
            .into_iter()
            .find(|node| node.ip() == ip)
            .unwrap();
        assert_eq!(refreshed.tag().map(String::as_str), Some("refresh"));
        assert_eq!(refreshed.seq(), Some(2));
    }

    #[test]
    fn nodes_reap_removes_stale_nodes() {
        let nodes = Nodes::new();
        let stale = Ipv4Addr::new(192, 168, 0, 1);
        let fresh = Ipv4Addr::new(192, 168, 0, 2);

        nodes.add(stale, None, None, false);
        nodes.add(fresh, None, None, false);

        let stale_age =
            Duration::from_secs((MAX_SILENT_INTERVALS + 1) * BROADCAST_INTERVAL.as_secs());
        nodes
            .data
            .write()
            .unwrap()
            .get_mut(&stale)
            .unwrap()
            .last_seen = Instant::now() - stale_age;

        nodes.reap();

        assert!(!nodes.test(&stale));
        assert!(nodes.test(&fresh));
    }
}
