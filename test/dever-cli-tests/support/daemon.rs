//! Lifetime of an explicitly requested test-owned shared compiler service.
use std::process::{Child, Command};
use std::time::Duration;

use dever_cli::toolchain::{Layout, cache_status};

pub struct TestDaemon(Child);

impl TestDaemon {
    pub fn start(layout: &Layout) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_deverd"))
            .env_clear()
            .current_dir(layout.root())
            .args(["--root", "."])
            .spawn()
            .unwrap();
        let mut daemon = Self(child);
        for _ in 0..100 {
            if cache_status(layout).is_ok() {
                return daemon;
            }
            assert!(
                daemon.0.try_wait().unwrap().is_none(),
                "test deverd exited early"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("test deverd did not become ready");
    }

    pub fn stop(&mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
