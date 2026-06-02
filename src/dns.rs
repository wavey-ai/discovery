use crate::{Nodes, DNS_CHECK_INTERVAL};
use if_addrs::get_if_addrs;
use rustdns::types::*;
use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, watch};
use tokio::time::{sleep, timeout, Duration};
use tracing::{info, warn};

const MAX_DNS_SEQUENCE: u32 = 100;
const MAX_CONSECUTIVE_DNS_MISSES: u32 = 3;

pub async fn discover(
    interfaces: Vec<&str>,
    dns_service: SocketAddr,
    domain: String,
    prefix: String,
    tags: Vec<String>,
) -> Result<
    (
        oneshot::Receiver<()>,
        oneshot::Receiver<()>,
        watch::Sender<()>,
        Arc<Nodes>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(());
    let (up_tx, up_rx) = oneshot::channel();
    let (fin_tx, fin_rx) = oneshot::channel();

    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(dns_service).await?;

    let nodes = Arc::new(Nodes::new());
    let nodes_clone = Arc::clone(&nodes);

    let mut own_ips = HashSet::new();
    for interface in interfaces {
        if let Some(ip) = get_ip(interface) {
            own_ips.insert(ip);
            info!("added own public ip {} to ignore list", ip.to_string());
        }
    }
    own_ips.insert(Ipv4Addr::new(127, 0, 0, 1));

    perform_dns_checks(&domain, &prefix, &tags, &socket, &nodes_clone, &own_ips).await;

    let _ = up_tx.send(());

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Shutdown signal received, stopping tasks");
                    break;
                }
                _ = sleep(DNS_CHECK_INTERVAL) => {
                    perform_dns_checks(&domain, &prefix, &tags, &socket, &nodes_clone, &own_ips).await;
                },
            }
        }

        let _ = fin_tx.send(());
    });

    Ok((up_rx, fin_rx, shutdown_tx, Arc::clone(&nodes)))
}

async fn perform_dns_checks(
    domain: &str,
    prefix: &str,
    tags: &[String],
    socket: &UdpSocket,
    nodes: &Arc<Nodes>,
    own_ips: &HashSet<Ipv4Addr>,
) {
    for tag in tags {
        let mut consecutive_misses = 0;
        for seq in 1..=MAX_DNS_SEQUENCE {
            let subdomain = format!("{}-{}-{}", prefix, tag, seq);
            match get_dns(domain, socket, &subdomain).await {
                Ok(Some(ip)) => {
                    consecutive_misses = 0;
                    if !nodes.test(&ip) && !own_ips.contains(&ip) {
                        info!("Discovered new node via DNS: {}", ip);
                    }

                    let is_self = own_ips.contains(&ip);
                    nodes.add(ip, Some(tag.to_owned()), Some(seq), is_self);
                }
                Ok(None) => {
                    consecutive_misses += 1;
                    info!(
                        "No DNS results subdomain={} domain={} consecutive_misses={}",
                        subdomain, domain, consecutive_misses
                    );
                    if consecutive_misses >= MAX_CONSECUTIVE_DNS_MISSES {
                        break;
                    }
                }
                Err(e) => {
                    warn!("Error querying {}: {}", subdomain, e);
                    break;
                }
            }
        }
    }
}

async fn get_dns(
    domain: &str,
    socket: &UdpSocket,
    subdomain: &str,
) -> io::Result<Option<Ipv4Addr>> {
    let mut m = Message::default();
    m.add_question(
        &format!("{}.{}", subdomain, domain),
        Type::A,
        Class::Internet,
    );
    m.add_extension(Extension {
        payload_size: 4096,
        ..Default::default()
    });

    let question = m.to_vec()?;
    socket.send(&question).await?;

    let mut resp = [0; 4096];
    let len = timeout(Duration::new(5, 0), socket.recv(&mut resp)).await??;

    let answer = Message::from_slice(&resp[0..len])?;

    for r in answer.answers {
        if let Resource::A(ip) = r.resource {
            if !ip.is_loopback() {
                return Ok(Some(ip.into()));
            }
        }
    }

    Ok(None)
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
