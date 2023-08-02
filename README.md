# `ksync` - an okay file synchronisation solution
`ksync` is a simple, immutable, file synchronisation solution written in Rust, using the [`tokio`](https://tokio.rs/) async framework, and the [`sled`](https://sled.rs/) database.  It aims to be a simple to use, easy to configure solution for syncing your files between devices.

### ⚠️ WARNING ⚠️
`ksync` is alpha software, and it should not be relied on in-production or to safely manage any critical information. If you do use `ksync` in this way, please make sure to manually create backups of your data just in-case of failure.

Additionally, `ksync` currently does not provide any sort of authentication, nor are it's communications encrypted, so do **not** expose a `ksync` server to the internet.

# Motivation
Most file synchronisation solutions that I found while looking for a way to sync my password database between my devices seem to be used to keep 2 folders, on the client and the server, in sync. 

While this is perfectly fine (and honestly would have worked for my purposes), this means that these solutions are limited by the host filesystem, namely:

1. rollbacks require either using snapshots (e.g. via `btrfs`), or manually archiving the sync folder.
2. compression and file de-duplication are reliant on the host filesystem to work, otherwise are non-existent. 

`ksync` aims to - though currently does not (see the [roadmap](ROADMAP.md)) solve the first problem through it's immutable design; whenever the filesystem tree is updated, it will create a new tree, and old trees will be able to be accessed simply by going back through a list of instances of the filesystem.

The second problem is already solved by `ksync`. While compression is not yet enabled, `sled` does support it, so it should simply be a case of enabling it in the codebase (will be done when the need arises). File de-duplication is already solved, as "objects" (pieces of data stored on the server), are indexed via a hash of their contents. This means that 2 files that have the same contents will occupy the same object.

# Usage
The basic usage of `ksync` is as follows:
```sh
# start a ksync daemon with a given configuration
ksync daemon -c [CONFIG FILE]
# interact with a ksync server with the command line interface
ksync cli [IP ADDRESS] [METHOD] [ARGS...]
```

## Command-line interface
Currently, `ksync` exposes only a few commands:
```sh
# insert a file into the database
ksync cli 127.0.0.1:8080 insert --from example/test.txt --to /files/test.txt
# retrieve a file from the database
ksync cli 127.0.0.1:8080 get --from /files/test.txt --to example/test.txt
# get a list of files from the database in the format of `<PATH>: <HASH> @ <DATE-TIME>`
ksync cli 127.0.0.1:8080 get-listing 
# clear the database of a given server
ksync cli 127.0.0.1:8080 clear
```

You can run `ksync -h` to see basic usage information. You can also ask for help for a given subcommand in the same way, e.g. `ksync cli 127.0.0.1:8080 get -h`.

# Configuration
The configuration for `ksync` is very simple (both by design, and because it is so early in it's development). 

**__Note:__** while there are 2 example configuration files, you can have a single `ksync` instance act as both a server and a synchronisation client; this could be useful if you want, for example, to use a directory on the same machine as an interface to the database; this will duplicate the information found inside the server's database into that folder, and will synchronise the two.

## Server
See [example/server.toml](example/server.toml) for the example configuration. Server configuration is specified inside of the `[server]` block.

* `addr` - the socket address the server will bind to, e.g.g `127.0.0.1:8080`.
* `db` - path to the server's files database.

## Sync Client
See [example/client.toml](example/client.toml) for the example configuration. Synchronisation client configuration is specified inside of the `[sync]` block.

* `remote` - the socket address of a `ksync` server to connect to/sync with.
* `resync_time` - the time (in seconds) between automatically re-syncing with the server
* `point` - the point to synchronise data to/from
    * `dir` - the directory to synchronise

# License
`ksync` is licensed under the **GNU General Public License, version 3 or later**; please see [LICENSE](LICENSE) for more details