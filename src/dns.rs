use rustdns::types::*;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;

pub struct Dns {
    domain: String,
    prefix: String,
    regions: Vec<String>,
    dns_service: SocketAddr, // TODO: support multiple?
}

impl Dns {
    pub fn new(
        dns_service: SocketAddr,
        domain: String,
        prefix: String,
        regions: Vec<String>,
    ) -> Self {
        Dns {
            domain,
            prefix,
            regions,
            dns_service,
        }
    }

    async fn get(&self, socket: &UdpSocket, subdomain: String) -> io::Result<Option<String>> {
        let mut m = Message::default();
        m.add_question(
            &format!("{}.{}", subdomain, self.domain),
            Type::A,
            Class::Internet,
        );
        m.add_extension(Extension {
            // Optionally add a EDNS extension
            payload_size: 4096, // which supports a larger payload size.
            ..Default::default()
        });

        let question = m.to_vec()?;
        socket.send(&question).await?;

        let mut resp = [0; 4096];
        let len = timeout(Duration::new(5, 0), socket.recv(&mut resp)).await??;

        let answer = Message::from_slice(&resp[0..len])?;

        for r in answer.answers {
            return Ok(Some(r.resource.to_string()));
        }

        Ok(None)
    }

    pub async fn all(&self) -> io::Result<HashMap<String, Vec<String>>> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(self.dns_service).await?; // Google's Public DNS Servers

        let mut res = HashMap::new();
        for r in &self.regions {
            let mut n = 0;
            let mut ips = Vec::new();

            loop {
                n += 1;
                let subdomain = format!("{}-{}-{}", self.prefix, r, n);
                match &self.get(&socket, subdomain.to_string()).await {
                    Ok(r) => {
                        if let Some(ip) = r {
                            ips.push(ip.to_string());
                        } else {
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("Error querying {}: {}", subdomain, e);
                    }
                }
            }

            res.insert(r.clone(), ips);
        }

        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_udp() {
        let domain = String::from("wavey.io");
        let regions = vec![String::from("uk-lon")];
        let prefix = String::from("live");

        let addr: SocketAddr = ([8, 8, 8, 8], 53).into();

        let dns = Dns::new(addr, domain, prefix, regions);

        let res = dns.all().await;
    }
}
