//! Port forwarding for the test servers: what sshd does for `ssh -L` and
//! `ssh -R`, enough of it to tunnel through.
//!
//! Shared by the integration tests and `examples/dev_sshd.rs`.

use parking_lot::Mutex;
use russh::server::{ChannelOpenHandle, Handle, Msg};
use russh::Channel;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// Listeners `tcpip-forward` opened, by address and port.
type Listeners = HashMap<(String, u32), JoinHandle<()>>;

/// The server's side of forwarding, for one client connection.
#[derive(Clone, Default)]
pub struct Forwarding {
    listeners: Arc<Mutex<Listeners>>,
}

impl Forwarding {
    /// `direct-tcpip`: connect to the target and pipe the channel to it. A
    /// target that does not answer refuses the channel.
    pub fn direct(&self, channel: Channel<Msg>, host: &str, port: u32, reply: ChannelOpenHandle) {
        let target = (host.to_string(), port as u16);
        tokio::spawn(async move {
            let Ok(mut socket) = TcpStream::connect(target).await else {
                reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
                return;
            };
            reply.accept().await;
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
        });
    }

    /// `tcpip-forward`: listen on the address, and open a `forwarded-tcpip`
    /// channel to the client for every connection. Port 0 picks one.
    pub async fn listen(&self, address: &str, port: &mut u32, handle: Handle) -> bool {
        let Ok(listener) = TcpListener::bind((address, *port as u16)).await else {
            return false;
        };
        let Ok(local) = listener.local_addr() else {
            return false;
        };
        *port = u32::from(local.port());
        let bound = (address.to_string(), *port);
        let task = tokio::spawn({
            let (address, port) = bound.clone();
            async move {
                while let Ok((mut socket, peer)) = listener.accept().await {
                    let handle = handle.clone();
                    let address = address.clone();
                    tokio::spawn(async move {
                        let Ok(channel) = handle
                            .channel_open_forwarded_tcpip(
                                address,
                                port,
                                peer.ip().to_string(),
                                u32::from(peer.port()),
                            )
                            .await
                        else {
                            return;
                        };
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                    });
                }
            }
        });
        self.listeners.lock().insert(bound, task);
        true
    }

    /// `cancel-tcpip-forward`: the listener is gone when this returns.
    pub async fn cancel(&self, address: &str, port: u32) -> bool {
        let task = self.listeners.lock().remove(&(address.to_string(), port));
        match task {
            Some(task) => {
                task.abort();
                let _ = task.await;
                true
            }
            None => false,
        }
    }
}
