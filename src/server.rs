use std::io;
use tokio::net::UdpSocket;

pub async fn run_server(addr: &str) -> io::Result<()> {
    let socket = UdpSocket::bind(addr).await?;
    println!("Server running on {}", addr);

    let mut buf = [0; 1024];

    loop {
        let (len, addr) = socket.recv_from(&mut buf).await?;
        println!("Received from {}: {:?}", addr, &buf[..len]);

        let response = b"hello";
        socket.send_to(response, addr).await?;
    }
}
