use discovery::dns::Dns;
use std::collections::HashSet;
use std::net::SocketAddr;
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
        regions: String,

        #[structopt(long, default_value = "8.8.8.8:53")]
        dns_server: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Command::from_args();

    match args {
        Command::Dns {
            dns_server,
            domain,
            prefix,
            regions,
        } => {
            let dns_server: SocketAddr = dns_server.parse()?;
            let regions: Vec<String> = regions.split(',').map(|s| s.to_string()).collect();
            let mut uniq_ips = HashSet::new();

            if let Ok(res) = Dns::new(dns_server, domain, prefix, regions).all().await {
                for (_, ips) in res {
                    uniq_ips.extend(ips.into_iter().map(|ip| ip.to_string()));
                }

                let all_ips: Vec<_> = uniq_ips.into_iter().collect();
                println!("{}", all_ips.join(" "));
            }
        }
    }

    Ok(())
}
