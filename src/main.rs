// NRS - an NVDA remote relay server
// Don't warn about unused imports for now.
#![allow(unused_imports)]

extern crate tokio;
extern crate tokio_rustls;
extern crate serde_json;

use std::collections::HashMap;
use std::fs::File;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use std::io::{ self, BufReader, Seek};
use std::time::Duration;
// use tokio::prelude::*;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime;
use tokio::sync::RwLock;
use tokio::stream::{Stream, StreamExt};
use tokio::time::timeout;
use tokio_rustls::rustls::{ Certificate, NoClientAuth, PrivateKey, ServerConfig };
use tokio_rustls::rustls::internal::pemfile::{ certs, pkcs8_private_keys, rsa_private_keys };
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::codec::{Framed, LinesCodec, LinesCodecError};
// Import stream and sink ext traits
// for functions like send, send_all (sink) and next, (stream).
// The stream extensions are from tokio, but the sink ones are from futures. Hmmmm.
use futures::SinkExt;


// No arg parsing or config yet, just load the cert given a hardcoded name and accept connections; more later.

/*
Using pkcs8_keys, the self-signed cert I have from the add-on finds no keys in the pem file.
rsa_private_keys works though, and no rsa key shows up with 'openssl x509 -in cert.pem -text'
Instead, openssl rsa -in cert.pem -text' shows one, so for now I'll use that.
*/

/// Shorthand alias for our global hash map of sessions.
type GlobalSessionsHashmap<'kl> = Arc<RwLock<HashMap<&'kl str, &'kl Session<'kl>>>>;
/// Alias for a lines-codec, tls-wrapped tcp connection.
type CONNECTION = Framed<TlsStream<TcpStream>, LinesCodec>;
/// Shorthand for an NVD remote session, wrapped in an RwLock and an arc for sharing across threads.
type SESSION<'kl> = Arc<RwLock<Session<'kl>>>;


// Structs in our main file, raa. (For now)


/// NVDA remote client modes
enum ClientControlMode {
    /// A client that will control another.
    Master,
    /// A client that will itself be controlled by another.
    Slave,
}

/// A connected NVDA remote client.
struct Client<'kl> {
    /// A client's line-framed, tls-wrapped tcp connection.
    connection: CONNECTION,
    /// The NVDA remote session this client is associated with.
    session: &'kl Session<'kl>,
    /// The NVDA remote protocol version this client says it's using.
    // When / if there are ever more than 255 (!) protocol versions we'll be sure to change this type immediately.
    // (And is i8 even best choice? Can't imagine where we'd have negative...)
    proto_version: i8,
    /// A client's current control mode.
    mode: ClientControlMode,
    // Clients need an arc<RwLock<SharedState>> or actually arc RwLock to their session.
    // Then probably new constructor that acquires lock on session
    // adds client to session (I guess on construct is fine since they need state / session ref anyways)
    // and drop impl that locks session again and removes client.
}

/// An NVDA Remote control session.
struct Session<'kl> {
    /// The global hash map of sessions.
    /// This is used by the drop impl to remove a session once it goes out of scope.
    // Is this 'idiomatic' / good practice / wil achieve the result I want cleanly? We'll find out.
    // Investigate if string references rather than owned is good here (I suspect not, refs should mean... lifetimes somewhere? hm.)
    sessions: &'kl GlobalSessionsHashmap<'kl>,
    /// The session's user-defined key, for bookkeeping purposes
    /// so the drop impl can remove it from the global list of active sessions.
    // The key should live as long as an instance exists.
    key: &'kl str,
    /// List of clients connected to this session.
    clients: Vec<Client<'kl>>,
}

impl<'kl> Session<'kl> {
    /// Session constructor.
    fn new (sessions: &'kl GlobalSessionsHashmap<'kl>, key: &'kl str) -> Session<'kl> {
        let session = Session {
            sessions,
            // Same with this string, though it'd be nice to just have it once in memory, might need to clone. Will see what compiler has to say about it.
            key,
            // Type is annotated on the struct field, can I just say Vec::new()?
            clients: Vec::new(),
        };
        // In the future, consider changing what global sessions are keyed by to a saulted hash of the actual key instead of the key itself.
        session
    }
}

impl Drop for Client<'_> {
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
    // let  sessions: GlobalSessionsHashmap = Arc::new(RwLock::new(HashMap::new()));
    // I believe the session global list should stick around, at least until main ends. As they're created, sessions will .clone() it and save so they have a reference.
    // My first future! (even if it is adapted from an example)
    // This is getting long enough where it's starting to make sense to functionize it soon.
    let server_f = async {
        // Create the socket listener for the server.
        let mut listener = TcpListener::bind(&addr).await?;
        println!("Server listening on {}.", &addr);
        loop {
            // Process incomming connections,and accept them as TLS.
            let (stream, peer_addr) = listener.accept().await?;
            println!("New connection established from {}.", &peer_addr);
            // Clone acceptor, so it's overridden with async move. (I think)
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                // Accepts and initiates as tls.
                match acceptor.accept(stream).await {
                    Err(e) => {
                        eprintln!("Error when accepting TLS connection for {}: {:?}", peer_addr, e);
                    },
                    Ok(stream) => {
                        // Call code here that initializes the connection:
                        // expect a line of JSON describing the client's control mode and chosen session key.
                        
                    }
                };
                
                Ok(()) as io::Result<()>
            });
        };
    };
    // Make the runtime run our main listening and connection accepting task.
    rt.block_on(server_f)
    // Does the above return an ok result? Let's comment this out and find out.
    // Ok(())
}
