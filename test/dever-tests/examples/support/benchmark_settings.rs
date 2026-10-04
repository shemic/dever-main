use std::path::Path;

use serde_json::{Map, Value};

pub struct Settings(Map<String, Value>);

impl Settings {
    pub fn load() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("cannot locate benchmark executable: {error}"))?;
        let root = executable
            .parent()
            .ok_or("benchmark executable has no parent")?;
        Self::read(&root.join("config/setting.json"))
    }

    pub fn check() -> Result<(), String> {
        println!("{{\"workers\":{}}}", Self::load()?.workers()?);
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let mut document: Map<String, Value> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid {}: {error}", path.display()))?;
        let Some(Value::Object(fields)) = document.remove("benchmark") else {
            return Err("config/setting.json must contain a benchmark object".into());
        };
        for key in fields.keys() {
            if !matches!(
                key.as_str(),
                "workers"
                    | "connections"
                    | "task_capacity"
                    | "timeout_ms"
                    | "lifetime_ms"
                    | "http2_streams"
                    | "http2_stream_window_bytes"
                    | "http2_connection_window_bytes"
                    | "cert_path"
                    | "key_path"
                    | "orm_count"
            ) {
                return Err(format!("unknown benchmark setting '{key}'"));
            }
        }
        Ok(Self(fields))
    }

    pub fn integer(
        &self,
        name: &str,
        default: usize,
        minimum: usize,
        maximum: usize,
    ) -> Result<usize, String> {
        let value = match self.value(name) {
            None => default,
            Some(value) => value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| format!("benchmark.{name} must be an unsigned integer"))?,
        };
        if !(minimum..=maximum).contains(&value) {
            return Err(format!(
                "benchmark.{name} must be between {minimum} and {maximum}"
            ));
        }
        Ok(value)
    }

    pub fn workers(&self) -> Result<usize, String> {
        self.integer("workers", 1, 1, 64)
    }

    pub fn value(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }
}
