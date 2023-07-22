mod files;

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
struct Cmdline {
    #[command(subcommand)]
    command: Command
}

#[derive(clap::Subcommand)]
enum Command {
    Add {
        file: PathBuf,
    },
    ListTree,
    ListLinks,
    GetLink { file: PathBuf }
}

fn print_files(files: &files::Files) -> sled::Result<()> {
    println!("objects:");
    for entry in files.objects() {
        let (object, _) = entry?;

        println!("  * {}", object.hex());
    }

    println!("links:");
    for entry in files.links() {
        let (name, object) = entry?;

        println!("  * '{}' -> {}", name, object.hex());
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // let cmdline = Cmdline::parse();
    let files = files::Files::open("/tmp/test-files")?;
    files.clear()?;

    print_files(&files)?;
    println!();

    let name = "test_file";
    let object = files.insert(name, b"Hello, World!")?;
    log::info!("inserted file '{name}' (object {})\n", object.hex());

    let object = files.lookup(name)?.unwrap();
    let data = files.get(&object)?;
    let string = String::from_utf8_lossy(&data[..]);
    log::info!("'{name}' stored at {}: \"{string}\"", object.hex());
    println!();

    print_files(&files)?;
    println!();

    let object = files.insert(name, b"Hello, World! but EDITED")?;
    log::info!("updated file '{name}' (object {})\n", object.hex());

    print_files(&files)?;
    println!();

    let object = files.lookup(name)?.unwrap();
    let data = files.get(&object)?;
    let string = String::from_utf8_lossy(&data[..]);
    log::info!("'{name}' stored at {}: \"{string}\"", object.hex());
    println!();

    Ok(())
}
