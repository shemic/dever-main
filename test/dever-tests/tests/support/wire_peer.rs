// Independent test targets use different subsets of the shared HTTP/WS wire tools.
#![allow(dead_code)]

use dever_runtime::{bytes::Bytes, net};

// 测试独立观察 HTTP 分块和 WebSocket 帧，不使用被测协议引擎。
pub struct Peer {
    pub socket: net::Socket,
    buffered: Vec<u8>,
}

impl Peer {
    pub async fn connect(port: i64) -> Result<Self, String> {
        Self::new(net::connect_timeout("127.0.0.1", port, 1000).await?)
    }

    pub fn new(socket: net::Socket) -> Result<Self, String> {
        net::timeout(&socket, 1500)?;
        Ok(Self {
            socket,
            buffered: Vec::new(),
        })
    }

    pub async fn write(&self, bytes: &[u8]) -> Result<(), String> {
        net::write(&self.socket, &Bytes::new(bytes.to_vec())).await
    }

    async fn more(&mut self) -> Result<(), String> {
        let bytes = net::read(&self.socket, 4096)
            .await?
            .ok_or("unexpected peer EOF")?;
        self.buffered.extend_from_slice(bytes.values());
        Ok(())
    }

    pub async fn take(&mut self, count: usize) -> Result<Vec<u8>, String> {
        while self.buffered.len() < count {
            self.more().await?;
        }
        Ok(self.buffered.drain(..count).collect())
    }

    async fn through(&mut self, delimiter: &[u8]) -> Result<Vec<u8>, String> {
        loop {
            if let Some(index) = self
                .buffered
                .windows(delimiter.len())
                .position(|part| part == delimiter)
            {
                return self.take(index + delimiter.len()).await;
            }
            self.more().await?;
        }
    }

    pub async fn head(&mut self) -> Result<String, String> {
        String::from_utf8(self.through(b"\r\n\r\n").await?).map_err(|error| error.to_string())
    }

    pub async fn chunk(&mut self) -> Result<Vec<u8>, String> {
        let line = self.through(b"\r\n").await?;
        let length =
            usize::from_str_radix(std::str::from_utf8(&line[..line.len() - 2]).unwrap(), 16)
                .unwrap();
        let bytes = self.take(length).await?;
        assert_eq!(self.take(2).await?, b"\r\n");
        Ok(bytes)
    }

    pub async fn frame(&mut self) -> Result<(u8, bool, Vec<u8>), String> {
        let head = self.take(2).await?;
        let masked = head[1] & 128 != 0;
        let length = match head[1] & 127 {
            126 => u16::from_be_bytes(self.take(2).await?.try_into().unwrap()) as usize,
            127 => u64::from_be_bytes(self.take(8).await?.try_into().unwrap()) as usize,
            value => value as usize,
        };
        assert!(length <= 8 * 1024 * 1024, "bounded test frame");
        let mask = if masked {
            self.take(4).await?
        } else {
            vec![0; 4]
        };
        let mut bytes = self.take(length).await?;
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
        Ok((head[0], masked, bytes))
    }

    pub async fn send_frame(&self, opcode: u8, masked: bool, payload: &[u8]) -> Result<(), String> {
        assert!(payload.len() < 126, "small independent test frame");
        let mask = [17, 23, 31, 47];
        let mut wire = vec![opcode, payload.len() as u8 | if masked { 128 } else { 0 }];
        if masked {
            wire.extend_from_slice(&mask);
        }
        wire.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ if masked { mask[index % 4] } else { 0 }),
        );
        self.write(&wire).await
    }

    pub async fn upgrade(&mut self) -> Result<(), String> {
        self.write(b"GET /ws HTTP/1.1\r\nHost: test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").await?;
        let headers = self.head().await?.to_ascii_lowercase();
        assert!(headers.starts_with("http/1.1 101"), "{headers}");
        assert!(headers.contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="));
        Ok(())
    }
}
