#![feature(io_error_more)]

mod files;
mod server;
mod config;

use std::path::PathBuf;

use clap::Parser;
use files::Files;

use common::Path;

// fn print_files(files: &files::Files) -> sled::Result<()> {
//     println!("objects:");
//     for entry in files.objects() {
//         let (object, _) = entry?;

//         println!("  * {}", object.hex());
//     }

//     println!("links:");
//     for entry in files.links() {
//         let (name, object) = entry?;

//         println!("  * '{}' -> {}", name, object.hex());
//     }

//     Ok(())
// }

#[derive(Parser)]
struct Cmdline {
    #[arg(short, long)]
    config: PathBuf
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = Cmdline::parse();

    let config_str = tokio::fs::read_to_string(args.config).await?;
    let config = toml::from_str(&config_str)?;

    let server = server::Server::init(config).await?;

    server.run().await?;

    // let files = Files::open("/tmp/test-db-aaaaa")?;

    // let paths = &[
    //     Path::new("/foo")?,
    //     Path::new("/bar")?,
    //     Path::new("/bar/baz")?
    // ];

    // let root = Path::new("/")?;

    // for path in paths {
    //     // files.make_dir(path)?;
    //     println!("{path}");
    // }

    // let path = Path::new("/abc/def/ghi/jkl")?;
    // let file = Path::new("/test.txt")?;

    // files.make_dir_recursive(path)?;

    // files.insert(file, "Hello, World!")?;

    // if let Some(object) = files.lookup(file)? {
    //     let data = files.get(&object)?.to_vec();
    //     let string = std::str::from_utf8(&data)?;

    //     println!("data: {string}\n");
    // }

    // for ancestor in path.ancestors() {
    //     println!("{ancestor}:");
    //     files.ls(ancestor)?;
    // }

    // let path = common::Path::new("/abc/def/ghi")?;

    // for part in path.parts() {
    //     println!("{part}");
    // }

    Ok(())
}
