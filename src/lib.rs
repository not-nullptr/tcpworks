use futures::SinkExt;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UdpSocket,
    sync::mpsc,
};
use tokio_stream::StreamExt;
use tokio_util::codec::LengthDelimitedCodec;

pub trait IntoChannels {
    fn into_channels_with_buffer<I>(self, buffer: usize) -> (mpsc::Sender<I>, mpsc::Receiver<I>)
    where
        I: Serialize + for<'de> Deserialize<'de> + Send + 'static;

    fn into_channels<T: Serialize + for<'de> Deserialize<'de> + Send + 'static>(
        self,
    ) -> (mpsc::Sender<T>, mpsc::Receiver<T>)
    where
        Self: Sized,
    {
        self.into_channels_with_buffer(24)
    }
}

impl<T> IntoChannels for T
where
    T: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    fn into_channels_with_buffer<I>(self, buffer: usize) -> (mpsc::Sender<I>, mpsc::Receiver<I>)
    where
        I: Serialize + for<'de> Deserialize<'de> + Send + 'static,
    {
        let (our_tx, their_rx) = mpsc::channel(buffer);
        let (their_tx, mut our_rx) = mpsc::channel(buffer);

        let (reader, writer) = tokio::io::split(self);

        tokio::spawn(async move {
            let mut codec = LengthDelimitedCodec::builder()
                .length_field_type::<u64>()
                .new_read(reader);

            while let Some(Ok(bytes)) = codec.next().await {
                match bincode::serde::decode_from_slice(&bytes, bincode::config::standard()) {
                    Ok((msg, ..)) => {
                        if let Err(e) = our_tx.send(msg).await {
                            log::warn!("receiver dropped: {e}");
                            break;
                        }
                    }

                    Err(e) => {
                        log::warn!("failed to decode message: {e}");
                        break;
                    }
                }
            }
        });

        tokio::spawn(async move {
            let mut codec = LengthDelimitedCodec::builder()
                .length_field_type::<u64>()
                .new_write(writer);

            while let Some(msg) = our_rx.recv().await {
                match bincode::serde::encode_to_vec(&msg, bincode::config::standard()) {
                    Ok(bytes) => {
                        if let Err(e) = codec.send(bytes.into()).await {
                            log::warn!("sender dropped: {e}");
                            break;
                        }
                    }
                    Err(e) => {
                        log::warn!("failed to encode message: {e}");
                        break;
                    }
                }
            }
        });

        (their_tx, their_rx)
    }
}

type UdpMessagePair<T> = (T, SocketAddr);

pub fn udp_into_channels<T>(
    socket: UdpSocket,
) -> (
    mpsc::Sender<UdpMessagePair<T>>,
    mpsc::Receiver<UdpMessagePair<T>>,
)
where
    T: Serialize + for<'de> Deserialize<'de> + Send + 'static,
{
    let (our_tx, their_rx) = mpsc::channel(24);
    let (their_tx, mut our_rx) = mpsc::channel(24);

    let socket = Arc::new(socket);
    let recv_socket = socket.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match recv_socket.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    match bincode::serde::decode_from_slice(
                        &buf[..len],
                        bincode::config::standard(),
                    ) {
                        Ok((msg, ..)) => {
                            if let Err(e) = our_tx.send((msg, addr)).await {
                                log::warn!("receiver dropped: {e}");
                                break;
                            }
                        }
                        Err(e) => {
                            log::warn!("failed to decode message: {e}");
                            continue;
                        }
                    }
                }
                Err(e) => {
                    log::warn!("failed to receive from socket: {e}");
                    break;
                }
            }
        }
    });

    tokio::spawn(async move {
        while let Some((msg, addr)) = our_rx.recv().await {
            match bincode::serde::encode_to_vec(&msg, bincode::config::standard()) {
                Ok(bytes) => {
                    if let Err(e) = socket.send_to(&bytes, &addr).await {
                        log::warn!("failed to send to {addr}: {e}");
                    }
                }
                Err(e) => {
                    log::warn!("failed to encode message: {e}");
                    continue;
                }
            }
        }
    });

    (their_tx, their_rx)
}
