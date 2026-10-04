//! Application deployment targets, independent of the compiler host identity.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
pub enum BuildTarget {
    #[serde(rename = "linux-x86_64")]
    LinuxX86_64,
    #[serde(rename = "linux-aarch64")]
    LinuxAarch64,
}

impl BuildTarget {
    pub fn host() -> Result<Self, String> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Ok(Self::LinuxX86_64),
            ("linux", "aarch64") => Ok(Self::LinuxAarch64),
            _ => Err("native executable runtime packs are not available for this platform".into()),
        }
    }

    pub fn platform(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "linux-x86_64",
            Self::LinuxAarch64 => "linux-aarch64",
        }
    }

    pub fn triple(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "x86_64-unknown-linux-gnu",
            Self::LinuxAarch64 => "aarch64-unknown-linux-gnu",
        }
    }
}

impl FromStr for BuildTarget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "linux-x86_64" => Ok(Self::LinuxX86_64),
            "linux-aarch64" => Ok(Self::LinuxAarch64),
            _ => Err(format!(
                "unsupported build target '{value}'; expected linux-x86_64 or linux-aarch64"
            )),
        }
    }
}
