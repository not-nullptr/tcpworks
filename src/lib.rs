use futures::SinkExt;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
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
                if let Ok((msg, ..)) =
                    bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                {
                    if let Err(e) = our_tx.send(msg).await {
                        eprintln!("Receiver dropped: {e}");
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
                if let Ok(bytes) = bincode::serde::encode_to_vec(&msg, bincode::config::standard())
                {
                    if let Err(e) = codec.send(bytes.into()).await {
                        eprintln!("Sender dropped: {e}");
                        break;
                    }
                }
            }
        });

        (their_tx, their_rx)
    }
}
