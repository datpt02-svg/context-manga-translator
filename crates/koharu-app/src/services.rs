//! Service lifecycle management for Python sidecar services.
//!
//! Spawns and manages third-party inference services (Unlimited-OCR, AnyText2)
//! as child processes.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;

use anyhow::{Context, Result};

/// A handle to a running service child process.
/// When dropped, the child is killed and we log its last output.
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
    pub port: u16,
    pub startup_timeout_secs: u16,
    /// Absolute path to the service directory.
    pub dir: String,
}

impl ServiceSpec {
    fn spawn(&self) -> Result<ServiceChild> {
        let mut cmd = Command::new("uv");
        cmd.args(["run", "python", "server.py"])
            .current_dir(&self.dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PORT", self.port.to_string());

        // Inject ANYTEXT2_REPO_DIR pointing to the parent of services/anytext2
        // so the server can import ms_wrapper.py from the repo root.
        if self.name == "anytext2" {
            if let Some(parent) = PathBuf::from(&self.dir).parent() {
                cmd.env("ANYTEXT2_REPO_DIR", parent);
            }
        }

        tracing::info!("[{}] spawning: uv run python server.py", self.name);

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "failed to spawn {} service (cwd: {}, uv run python server.py)",
                self.name, self.dir
            )
        })?;

        // Take stdout/stderr so they don't fill OS buffers.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let name = self.name;

        // Log child output asynchronously.
        if let Some(mut reader) = stdout {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut buf = [0u8; 1024];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                        for line in s.lines() {
                            tracing::info!("[{name}] {line}");
                        }
                    }
                }
            });
        }
        if let Some(mut reader) = stderr {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut buf = [0u8; 1024];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                        for line in s.lines() {
                            tracing::info!("[{name}] {line}");
                        }
                    }
                }
            });
        }

        Ok(ServiceChild {
            name: self.name,
            child: Some(child),
        })
    }
}

/// Known service specs. `dir` is resolved at spawn time via an env var or
/// the path relative to the running binary's discovery root.
pub fn unlimited_ocr_spec() -> &'static ServiceSpec {
    static SPEC: OnceLock<ServiceSpec> = OnceLock::new();
    SPEC.get_or_init(|| ServiceSpec {
        name: "unlimited-ocr",
        port: 7862,
        startup_timeout_secs: 120,
        dir: resolve_service_dir("services/unlimited-ocr"),
    })
}

pub fn anytext2_spec() -> &'static ServiceSpec {
    static SPEC: OnceLock<ServiceSpec> = OnceLock::new();
    SPEC.get_or_init(|| ServiceSpec {
        name: "anytext2",
        port: 7863,
        startup_timeout_secs: 180,
        dir: resolve_service_dir("services/anytext2"),
    })
}

/// Resolve the service directory path.
/// Tries: `KOHARU_ROOT` env var → parent of binary's dir → target/debug (dev).
fn resolve_service_dir(relative: &str) -> String {
    if let Ok(root) = std::env::var("KOHARU_ROOT") {
        let p = PathBuf::from(root).join(relative);
        if p.exists() {
            return p.to_string_lossy().to_string();
        }
    }
    // Walk up from the binary to find the project root (has Cargo.toml).
    if let Ok(exe) = std::env::current_exe() {
        let mut p = exe.parent().unwrap_or(&exe).to_path_buf();
        loop {
            if p.join("Cargo.toml").exists() || p.join("package.json").exists() {
                let candidate = p.join(relative);
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
            if !p.pop() {
                break;
            }
        }
    }
    // Fallback: relative from cwd
    relative.to_string()
}

fn port_reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(200),
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

    tracing::info!("{} not reachable on port {}, spawning...", spec.name, spec.port);
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
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_valid() {
        assert_eq!(unlimited_ocr_spec().port, 7862);
        assert_eq!(anytext2_spec().port, 7863);
    }
}
