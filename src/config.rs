//! CLI configuration and argument parsing.

use clap::Parser;

/// WebRTC to UDP proxy for CS 1.6 / Half-Life servers
#[derive(Parser, Debug, Clone)]
#[command(name = "webxash3d-proxy")]
#[command(about = "WebRTC to UDP proxy for CS 1.6 / Half-Life servers")]
pub struct Config {
    /// CS 1.6 server address (e.g., 192.168.1.100:27015)
    #[arg(short, long, env = "GAME_SERVER")]
    pub server: String,

    /// Port to listen for WebSocket/HTTP connections
    #[arg(short, long, default_value = "27016", env = "LISTEN_PORT")]
    pub port: u16,

    /// Host to bind to
    #[arg(long, default_value = "0.0.0.0", env = "LISTEN_HOST")]
    pub host: String,

    /// Public IP for ICE candidates (for NAT traversal)
    #[arg(long, env = "PUBLIC_IP")]
    pub public_ip: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    /// Path to valve.zip game package file
    #[arg(long, env = "PACKAGE_ZIP")]
    pub package_zip: Option<String>,

    /// Path to serve static files from (development mode, overrides embedded assets)
    #[arg(long, env = "STATIC_DIR", hide = true)]
    pub static_dir: Option<String>,

    /// Game directory name (e.g., "cstrike", "valve")
    #[arg(long, default_value = "cstrike", env = "GAME_DIR")]
    pub game_dir: String,

    /// Fixed UDP port range for WebRTC ICE (format "start-end"), for opening
    /// ports on firewalls/NAT. Empty means ephemeral ports.
    #[arg(long, env = "UDP_PORT_RANGE")]
    pub udp_port_range: Option<String>,

    /// Extra console commands to execute on client start (comma-separated)
    #[arg(long, env = "CONSOLE_COMMANDS")]
    pub console_commands: Option<String>,
}

impl Config {
    /// Get the listen address as a string
    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Get console commands as a vector
    pub fn get_console_commands(&self) -> Vec<String> {
        self.console_commands
            .as_ref()
            .map(|s| s.split(',').map(|cmd| cmd.trim().to_string()).collect())
            .unwrap_or_default()
    }

    /// Check if using embedded assets (no `static_dir` override)
    pub fn use_embedded_assets(&self) -> bool {
        self.static_dir.is_none()
    }

    /// Parse "start-end" UDP port range for WebRTC ICE
    pub fn udp_port_range_parsed(&self) -> Option<(u16, u16)> {
        let raw = self.udp_port_range.as_ref()?;
        let mut parts = raw.split('-');
        let start = parts.next()?.trim().parse::<u16>().ok()?;
        let end = parts.next()?.trim().parse::<u16>().ok()?;
        if start == 0 || end < start {
            return None;
        }
        Some((start, end))
    }
}
