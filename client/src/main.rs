use std::net::SocketAddr;
use std::path::PathBuf;

use tokio::net;

use common::proto;

use clap::Parser;

#[derive(Parser)]
struct Cmdline {
    addr: SocketAddr,
    #[command(subcommand)]
    cmd: Command
}

#[derive(Parser)]
enum Command {
    Insert {
        path: PathBuf
    },
    Get {
        path: PathBuf
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Cmdline::parse();

    let mut stream = net::TcpStream::connect("127.0.0.1:8080").await?;

    match args.cmd {
        Command::Insert { path } => {
            let data = tokio::fs::read(&path).await?;
            let name = path.to_str().unwrap();

            let request = proto::Packet {
                method: [b'I',b'N',b'S',b'E',b'R',b'T',0,0],
                data: bincode::serialize(&(name, data))?
            };

            request.write(&mut stream).await?;

            let response = proto::read_packet(&mut stream).await?;

            dbg!(&response);
            assert_eq!(response.method, b"OK\0\0\0\0\0\0".to_owned());
        },

        Command::Get { path } => {
            let name = path.to_str().unwrap();

            let request = proto::Packet {
                method: [b'G',b'E',b'T',0,0,0,0,0],
                data: bincode::serialize(name)?
            };

            request.write(&mut stream).await?;

            let response = proto::read_packet(&mut stream).await?;
            dbg!(&response);

            assert_eq!(response.method, b"OK\0\0\0\0\0\0".to_owned());

            tokio::fs::write(path, bincode::deserialize::<Vec<u8>>(&response.data)?).await?;
        }
    }

    Ok(())
}