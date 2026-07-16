//! Service lifecycle management for Python sidecar services.
//!
//! Spawns and manages third-party inference services (Unlimited-OCR, AnyText2)
//! as child processes. Exposes a `ServiceManager` singleton.

use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

/// A handle to a running service child process.
/// When dropped, the child is killed.
pub struct ServiceChild {
    name: &'static str,
    child: Option<Child>,
}

impl ServiceChild {
    pub fn name(&self) -> &str {
        self.name
    }
}

impl Drop for ServiceChild {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Shell command + args for a known service.
#[derive(Debug, Clone)]
pub struct ServiceSpec {
    pub name: &'static str,
    /// Default port the service listens on.
    pub port: u16,
    /// How long (seconds) to wait for the service to become reachable.
    pub startup_timeout_secs: u16,
    /// Path to the service directory relative to the koharu project root.
    pub dir: &'static str,
    /// Extra environment variables for the service.
    pub env: &'static [(&'static str, &'static str)],
}

impl ServiceSpec {
    fn spawn(&self) -> Result<ServiceChild> {
        let mut cmd = Command::new("uv");
        cmd.args(["run", "python", "server.py"])
            .current_dir(self.dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PORT", self.port.to_string());

        for (k, v) in self.env {
            cmd.env(k, v);
        }

        let child = cmd.spawn().with_context(|| {
            format!("failed to spawn {} service (uv run)", self.name)
        })?;

        Ok(ServiceChild {
            name: self.name,
            child: Some(child),
        })
    }
}

/// Known service specs.
pub const UNLIMITED_OCR: ServiceSpec = ServiceSpec {
    name: "unlimited-ocr",
    port: 7862,
    startup_timeout_secs: 120,
    dir: "services/unlimited-ocr",
    env: &[],
};

pub const ANYTEXT2: ServiceSpec = ServiceSpec {
    name: "anytext2",
    port: 7863,
    startup_timeout_secs: 180,
    dir: "services/anytext2",
    env: &[],
};

/// Check if a TCP port on localhost is accepting connections.
fn port_reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(500),
    )
    .is_ok()
}

/// Spawn a service, wait for it to be reachable, and return a handle.
///
/// If the service is already running (port reachable), returns `Ok(None)`.
pub fn ensure_running(spec: &ServiceSpec) -> Result<Option<ServiceChild>> {
    if port_reachable(spec.port) {
        tracing::info!("{} already running on port {}", spec.name, spec.port);
        return Ok(None);
    }

    tracing::info!("spawning {} service...", spec.name);
    let child = spec.spawn()?;
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(spec.startup_timeout_secs as u64);

    loop {
        if port_reachable(spec.port) {
            tracing::info!("{} service is ready on port {}", spec.name, spec.port);
            return Ok(Some(child));
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!(
                "{} service did not start within {} seconds",
                spec.name,
                spec.startup_timeout_secs
            );
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_valid() {
        assert_eq!(UNLIMITED_OCR.port, 7862);
        assert_eq!(ANYTEXT2.port, 7863);
    }
}
