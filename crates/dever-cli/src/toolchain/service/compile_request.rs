use super::super::compilation::CompileRequest;
use super::super::release::Layout;

/// Submit source and compile-only bindings, never a client-chosen cache key or IR.
pub fn compile(layout: &Layout, request: &CompileRequest) -> Result<Vec<u8>, String> {
    #[cfg(unix)]
    {
        use super::{Operation, Request, Response, connect, read_frame, write_frame};
        use std::io::{Read, Write};
        use std::time::{Duration, Instant};
        let payload = request.encode()?;
        let (mut stream, token) = connect(layout)?;
        write_frame(
            &mut stream,
            &Request {
                token,
                operation: Operation::Compile {
                    bytes: payload.len() as u64,
                },
                installed: Vec::new(),
            },
        )?;
        let ready: Response = read_frame(&mut stream)?;
        if !ready.ok {
            return Err(ready
                .error
                .unwrap_or_else(|| "deverd rejected compilation".into()));
        }
        stream
            .write_all(&payload)
            .map_err(|error| format!("deverd compilation upload failed: {error}"))?;
        drop(payload);
        let mut download = CompilationDownload {
            stream: &mut stream,
            deadline: Instant::now() + Duration::from_secs(190),
        };
        let response: Response = read_frame(&mut download)?;
        if !response.ok {
            return Err(response
                .error
                .unwrap_or_else(|| "deverd compilation failed".into()));
        }
        let receipt = response
            .artifact
            .ok_or("deverd returned no compiled output identity")?;
        receipt.validate_compiled()?;
        let mut output = vec![0; receipt.bytes as usize];
        download
            .read_exact(&mut output)
            .map_err(|error| format!("deverd compilation download is incomplete: {error}"))?;
        if super::hex(ring::digest::digest(&ring::digest::SHA256, &output).as_ref())
            != receipt.sha256
        {
            return Err("deverd compiled output failed SHA-256 verification".into());
        }
        Ok(output)
    }
    #[cfg(not(unix))]
    {
        let _ = (layout, request);
        Err("trusted deverd compilation is not implemented for this platform".into())
    }
}

#[cfg(unix)]
struct CompilationDownload<'a> {
    stream: &'a mut std::os::unix::net::UnixStream,
    deadline: std::time::Instant,
}

#[cfg(unix)]
impl std::io::Read for CompilationDownload<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "compiled output download deadline exceeded",
                )
            })?;
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}
