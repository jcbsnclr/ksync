# Roadmap 
## Currently implemented features
* upload/download to/from server
* retrieve a list of files from the server, along with their metadata
* clear the server's database
* bi-direction (client <-> server) synchronisation
* file de-duplication

## Planned for `1.0.0` release
* rollback/forward changes to the filesystem
* support removing files/folders
* encrypted communication to/from server
* basic authentication via public key cryptography
* garbage collection (removing objects that are no longer referenced)
* automatically/manually produce `.tar.gz` backups of the server

## Long-term plans
* web interface
* Mandatory Access Control (MAC) system
    * support individual user authentication via public/private key cryptography
    * permissions system for users and files
* configurable sync behaviour
    * sync from client to server
    * sync from server to client
    * sync bi-directionally
* plugin system
    * potentially (?) using WebAssembly modules
* support multiple filesystem trees