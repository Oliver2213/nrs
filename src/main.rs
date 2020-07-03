// NRS - an NVDA remote relay server

extern crate tokio;
extern crate tokio_rustls;
extern crate serde_json;

use std::collections::HashMap;
use std::fs::File;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::io::{ self, BufReader, Seek};
use tokio::prelude::*;
use tokio::net::TcpListener;
use tokio::runtime;
use tokio::sync::RwLock;
use tokio_rustls::rustls::{ Certificate, NoClientAuth, PrivateKey, ServerConfig };
use tokio_rustls::rustls::internal::pemfile::{ certs, pkcs8_private_keys, rsa_private_keys };
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;


// No arg parsing or config yet, just load the cert given a hardcoded name and accept connections; more later.

/*
Using pkcs8_keys, the self-signed cert I have from the add-on finds no keys in the pem file.
rsa_private_keys works though, and no rsa key shows up with 'openssl x509 -in cert.pem -text'
Instead, openssl rsa -in cert.pem -text' shows one, so for now I'll use that.
*/


// Structs in our main file, raa.

/// NVDA remote client modes
enum ClientControlMode {
    /// A client that will control another.
    Master,
    /// A client that will itself be controlled by another.
    Slave,
}

/// A connected NVDA remote client.
struct Client {
    /// A client's associated tls-wrapped TCP connection.
    connection: TlsStream,
    /// A client's associated NVDA remote session.
    session: Arc<RwLock<Session>>,
    /// The NVDA remote protocol version this client says it's using.
    // When / if there are ever more than 255 (!) protocol versions we'll be sure to change this type immediately.
    // (And is i8 even best choice? Can't imagine where we'd have negative...)
    // is an option because... Maybe making client instances before they've said hello to us? Idk, might change.
    proto_version: Option<i8>,
    /// A client's current control mode.
    mode: Option<ClientControlMode>,
    // Clients need an arc<RwLock<SharedState>> or actually arc RwLock to their session.
    // Then probably new constructor that acquires lock on session
    // adds client to session (I guess on construct is fine since they need state / session ref anyways)
    // and drop impl that locks session again and removes client.
}

/// An NVDA Remote control session.
struct Session {
    /// The global hash map of sessions.
    /// This is used by the drop impl to remove a session once it goes out of scope.
    // Is this 'idiomatic' / good practice / wil achieve the result I want cleanly? We'll find out.
    // Also maybe consider the type alias thing, so I can just type global_sessions or something.
    // Also investigate if string references rather than owned is good here (I suspect not, refs should mean... lifetimes somewhere? hm.)
    sessions: Arc<RwLock<HashMap<&str, Client>>>,
    /// The session's user-defined key, for bookkeeping purposes
    /// so the drop impl can remove it from the global list of active sessions.
    key: String,
    /// List of clients connected to this session.
    clients: Vec<Client>,
}

impl Session {
    /// Session constructor.
    // Associated function.
    fn new (key: &str) -> Session {
        session {
            key,
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        
    }
}


fn main() -> std::io::Result<()> {
    const cert_fname: &'static str  = "cert.pem";
    let addr = "127.0.0.1:6837"
      .to_socket_addrs()?
      .next()
      .ok_or_else(|| io::Error::from(io::ErrorKind::AddrNotAvailable))?;
    // Load certificate and key from that file.
    let mut rt = runtime::Builder::new()
      .basic_scheduler()
      .enable_io()
      .build()?;
    let handle = rt.handle().clone();
    let mut tls_config = ServerConfig::new(NoClientAuth::new());
    let mut certbuf = BufReader::new(File::open(cert_fname)?);
    let certs = certs(&mut certbuf).expect("no certificates found!");
    // Aha! The seeking is necessary. Without it 0 keys again.
    certbuf.seek(std::io::SeekFrom::Start(0))?;
    //let keys = pkcs8_private_keys(&mut certbuf).expect("No private keys found!");
    let mut keys = rsa_private_keys(&mut certbuf).expect("No private keys found!");
    // Thinking about this a little... That file is gonna stay open for the entire execution. CHANGE THIS
    println!("Loaded certificate file. {} certs, {} keys.", certs.len(), keys.len());
    tls_config.set_single_cert(certs, keys.remove(0))
      .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));
    // Global map of sessions.
    // Inside an arc and an RwLock, so I can pass a cloned reference to sessions as they're created
    // and when dropped as they go out of scope later, they can remove themselves.
    // (Is this really how I should write this? Looks... Fine, I guess, just... long.)
    let  sessions: Arc<RwLock<HashMap<&str, Client>>> = Arc::new(RwLock::new(HashMap::new()));
    // I believe the session global list should stick around, at least until main ends. As they're created, sessions will .clone() it and save so they have a reference.
    // My first future! (even if it is adapted from an example)
    let server_f = async {
        // Create the socket listener for the server.
        let mut listener = TcpListener::bind(&addr).await?;
        // Code here to print / log bound address.
        loop {
            // Process incomming connections,and accept them as TLS.
            let (stream, peer_addr) = listener.accept().await?;
            // Clone acceptor, so it's overridden with async move. (I think)
            let acceptor = acceptor.clone();
            let accept_f = async move {
                // Accepts and initiates as tls.
                // check the result of what .accept returns, probably match. continue if ok, log if error.
                // acceptor.accept returns a future that will complete when the tls handshake does, so we need to .await it. (I think).
                let mut stream = acceptor.accept(stream).await?;
                Ok(()) as io::Result<()>
            };
            // Now await accept_f? Or maybe spawn it so we can keep handling incomming.
        }
    };
    // Make the runtime run our main future task thing.
    rt.block_on(server_f)
    // Does the above return an ok result? Let's comment this out and find out.
    // Ok(())
}
