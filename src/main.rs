use discovery::{dns::discover, vlan};
use std::collections::HashSet;
use std::net::{Shutdown, SocketAddr};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = "discovery", about = "A tool for discovering services")]
enum Command {
    Dns {
        #[structopt(long)]
        domain: String,

        #[structopt(long)]
        prefix: String,

        #[structopt(long)]
        tags: String,

        #[structopt(long, default_value = "8.8.8.8:53")]
        dns_server: String,
    },
    Vlan {},
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Command::from_args();

    match args {
        Command::Vlan {} => {
            let (_up, _fin, _shutodwn_tx, nodes) = vlan::discover().await.unwrap();
            while let Ok(ip) = nodes.rx().recv().await {
                dbg!(ip);
            }
        }
        Command::Dns {
            dns_server,
            domain,
            prefix,
            tags,
        } => {
            let dns_server: SocketAddr = dns_server.parse()?;
            let tags: Vec<String> = tags.split(',').map(|s| s.to_string()).collect();
            let mut uniq_ips = HashSet::new();

            let (up_rx, fin_rx, shutdown_rx, nodes) =
                discover(dns_server, domain, prefix, tags).await.unwrap();

            let _ = up_rx.await;

            for node in &nodes.all() {
                dbg!(node);
                uniq_ips.insert(node.ip());
            }

            let all_ips: Vec<_> = uniq_ips.into_iter().collect();
            println!(
                "{}",
                all_ips
                    .into_iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<String>>()
                    .join(" ")
            );

            let _ = shutdown_rx.send(());
        }
    }

    Ok(())
}
