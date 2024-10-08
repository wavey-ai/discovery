use crate::{Node, Nodes, BROADCAST_INTERVAL, DNS_CHECK_INTERVAL};
use if_addrs::get_if_addrs;
use rustdns::types::*;
use std::collections::HashSet;
use std::io;
use std::net::IpAddr;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, watch};
use tokio::time::{sleep, timeout, Duration};
use tracing::{debug, error, info, warn};

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
    let dns_service = dns_service.clone();
    let domain = domain.clone();
    let nodes_clone = Arc::clone(&nodes);

    let mut own_ips = HashSet::new();
    for interface in interfaces {
        if let Some(ip) = get_ip(interface) {
            own_ips.insert(ip);
            info!("added own public ip {} to ignore list", ip.to_string());
        }
    }
    own_ips.insert(Ipv4Addr::new(127, 0, 0, 1));

    perform_dns_checks(
        &dns_service,
        &domain,
        &prefix,
        &tags,
        &socket,
        &nodes_clone,
        &own_ips,
    )
    .await;

    let _ = up_tx.send(());

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    info!("Shutdown signal received, stopping tasks");
                    break;
                }
                _ = sleep(DNS_CHECK_INTERVAL) => {
                    perform_dns_checks(&dns_service, &domain, &prefix, &tags, &socket, &nodes_clone, &own_ips).await;
                },
            }
        }

        let _ = fin_tx.send(());
    });

    Ok((up_rx, fin_rx, shutdown_tx, Arc::clone(&nodes)))
}

async fn perform_dns_checks(
    dns_service: &SocketAddr,
    domain: &String,
    prefix: &String,
    tags: &[String],
    socket: &UdpSocket,
    nodes: &Arc<Nodes>,
    own_ips: &HashSet<Ipv4Addr>,
) {
    for tag in tags {
        let mut seq = 0;
        while seq < 100 {
            seq += 1;
            let subdomain = format!("{}-{}-{}", prefix, tag, seq);
            match get_dns(*dns_service, domain.clone(), socket, subdomain.to_string()).await {
                Ok(Some(ip)) => {
                    if !nodes.test(&ip) && !own_ips.contains(&ip) {
                        info!("Discovered new node via DNS: {}", ip);
                    }

                    let is_self = own_ips.contains(&ip);
                    // always add to update last seen
                    nodes.add(ip.to_owned(), Some(tag.to_owned()), Some(seq), is_self);
                }
                Ok(None) => {
                    info!("No DNS results subdomain={} domain={}", subdomain, domain);
                    break;
                }
                Err(e) => {
                    eprintln!("Error querying {}: {}", subdomain, e);
                    break;
                }
            }
        }
    }
}

async fn get_dns(
    dns_service: SocketAddr,
    domain: String,
    socket: &UdpSocket,
    subdomain: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_udp() {
        let domain = String::from("wavey.io");
        let tags = vec![String::from("uk-lon")];
        let prefix = String::from("live");

        let addr: SocketAddr = ([8, 8, 8, 8], 53).into();
    }
}
