#[cfg(all(feature = "external", target_os = "linux"))]
use tokio::io::AsyncReadExt;

pub(super) struct Observation {
    #[cfg(all(feature = "external", target_os = "linux"))]
    startup: tokio::net::unix::pipe::Receiver,
    #[cfg(all(feature = "external", target_os = "linux"))]
    information: tokio::net::unix::pipe::Receiver,
    #[cfg(all(feature = "external", target_os = "linux"))]
    information_bytes: Vec<u8>,
    #[cfg(all(feature = "external", target_os = "linux"))]
    release: Option<std::fs::File>,
    #[cfg(all(feature = "external", target_os = "linux"))]
    namespace: Option<tokio::io::unix::AsyncFd<dever_sandbox::CommandNamespace>>,
}

impl Observation {
    #[cfg(all(feature = "external", target_os = "linux"))]
    pub(super) fn new(observation: dever_sandbox::CommandObservation) -> Result<Self, String> {
        use tokio::net::unix::pipe::Receiver;
        Ok(Self {
            startup: Receiver::from_file(observation.startup).map_err(|error| error.to_string())?,
            information: Receiver::from_file(observation.namespace)
                .map_err(|error| error.to_string())?,
            information_bytes: Vec::new(),
            release: Some(observation.release),
            namespace: None,
        })
    }

    #[cfg(all(feature = "external", not(target_os = "linux")))]
    pub(super) fn new(_: dever_sandbox::CommandObservation) -> Result<Self, String> {
        Err("external commands require Linux".into())
    }

    #[cfg(all(feature = "external", target_os = "linux"))]
    async fn pin(&mut self, wrapper: u32) -> Result<(), String> {
        if self.namespace.is_some() {
            return Ok(());
        }
        let mut buffer = [0; 1024];
        loop {
            let count = self
                .information
                .read(&mut buffer)
                .await
                .map_err(|error| format!("cannot read command namespace identity: {error}"))?;
            if count == 0 {
                break;
            }
            if self.information_bytes.len() + count > 4096 {
                return Err("command namespace identity exceeds its byte limit".into());
            }
            self.information_bytes.extend_from_slice(&buffer[..count]);
        }
        let text = std::str::from_utf8(&self.information_bytes)
            .map_err(|_| "command namespace identity is not UTF-8")?;
        let node = crate::wire::parse(text)?;
        let pid = node
            .object()?
            .get("child-pid")
            .ok_or("command namespace identity is missing")?
            .int()?;
        let namespace = dever_sandbox::CommandNamespace::pin(
            i32::try_from(pid).map_err(|_| "invalid command namespace PID")?,
            wrapper,
        )?;
        self.namespace = Some(
            tokio::io::unix::AsyncFd::new(namespace)
                .map_err(|error| format!("cannot observe command namespace exit: {error}"))?,
        );
        Ok(())
    }

    pub(super) async fn started(&mut self, wrapper: u32) -> Result<(), String> {
        #[cfg(all(feature = "external", target_os = "linux"))]
        {
            self.pin(wrapper).await?;
            // EOF releases bwrap only after its namespace is pinned. Neither
            // normal cancellation nor a very short process can race PID reuse.
            drop(self.release.take());
            let mut status = Vec::new();
            (&mut self.startup)
                .take(4096)
                .read_to_end(&mut status)
                .await
                .map_err(|error| format!("cannot read command startup status: {error}"))?;
            match status.as_slice() {
                b"R" => Ok(()),
                [] => Err("external command sandbox failed before starting its guard".into()),
                status => {
                    let message = status
                        .strip_prefix(b"RE")
                        .or_else(|| status.strip_prefix(b"E"));
                    Err(match message {
                        Some(message) => format!(
                            "external command startup failed: {}",
                            String::from_utf8_lossy(message)
                        ),
                        None => "invalid external command startup status".into(),
                    })
                }
            }
        }
        #[cfg(not(all(feature = "external", target_os = "linux")))]
        {
            let _ = wrapper;
            Err("external commands require Linux".into())
        }
    }

    pub(super) async fn terminate(&mut self, wrapper: u32) -> Result<(), String> {
        #[cfg(all(feature = "external", target_os = "linux"))]
        {
            self.pin(wrapper).await?;
            let namespace = self
                .namespace
                .as_ref()
                .expect("namespace pinned before cleanup");
            namespace.get_ref().terminate()?;
            // pidfd readiness follows kernel namespace teardown, unlike pipe
            // EOF, which occurs while the init is still closing its files.
            let _exited = namespace
                .readable()
                .await
                .map_err(|error| format!("cannot wait for command namespace exit: {error}"))?;
            Ok(())
        }
        #[cfg(not(all(feature = "external", target_os = "linux")))]
        {
            let _ = wrapper;
            Err("external commands require Linux".into())
        }
    }
}
