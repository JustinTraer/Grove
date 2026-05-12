pub mod remote;

pub use remote::RemoteTmuxSession;

use std::collections::HashMap;
use std::io::Read;
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use ssh2::Session;

use crate::app::config::RemoteHost;

#[derive(Clone)]
pub struct SshClient {
    host: String,
    user: String,
    port: u16,
    identity_file: Option<String>,
}

impl SshClient {
    pub fn new(host: &str, user: &str, port: u16, identity_file: Option<&Path>) -> Self {
        Self {
            host: host.to_string(),
            user: user.to_string(),
            port,
            identity_file: identity_file.map(|p| p.to_string_lossy().to_string()),
        }
    }

    pub fn from_config(host_config: &RemoteHost) -> Result<Self> {
        let identity_file = host_config
            .identity_file
            .as_ref()
            .map(Path::new)
            .map(|p| p.to_path_buf());

        Ok(Self::new(
            &host_config.host,
            &host_config.user,
            host_config.port,
            identity_file.as_deref(),
        ))
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    fn tcp_connect(&self) -> Result<TcpStream> {
        let addr = format!("{}:{}", self.host, self.port);
        let tcp = TcpStream::connect(&addr).context(format!("Failed to connect to {}", addr))?;
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        tcp.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
        Ok(tcp)
    }

    fn authenticate(&self, session: &Session) -> Result<()> {
        if let Some(ref key_path) = self.identity_file {
            session
                .userauth_pubkey_file(&self.user, None, Path::new(key_path), None)
                .context("SSH public key authentication failed")?;
        } else if let Ok(_auth_sock) = std::env::var("SSH_AUTH_SOCK") {
            let mut agent = session.agent()?;
            agent.connect()?;
            agent.list_identities()?;
            session
                .userauth_agent(&self.user)
                .context("SSH agent authentication failed")?;
        } else {
            anyhow::bail!(
                "No authentication method available. Set SSH_AUTH_SOCK or provide identity_file"
            );
        }

        if !session.authenticated() {
            anyhow::bail!("SSH authentication failed");
        }

        Ok(())
    }

    pub fn connect(&self) -> Result<Session> {
        let tcp = self.tcp_connect()?;

        let mut session = Session::new()?;
        session.set_tcp_stream(tcp);
        session.handshake()?;
        self.authenticate(&session)?;
        Ok(session)
    }

    pub fn execute(&self, command: &str) -> Result<String> {
        let session = self.connect()?;

        let mut channel = session.channel_session()?;
        channel.exec(command)?;

        let mut output = String::new();
        channel.read_to_string(&mut output)?;

        channel.wait_close()?;
        let exit_status = channel.exit_status()?;

        if exit_status != 0 {
            anyhow::bail!("Command exited with status {}: {}", exit_status, output);
        }

        session.disconnect(None, "Done", None)?;
        Ok(output)
    }

    pub fn execute_with_exit_code(&self, command: &str) -> Result<(String, i32)> {
        let session = self.connect()?;

        let mut channel = session.channel_session()?;
        channel.exec(command)?;

        let mut output = String::new();
        channel.read_to_string(&mut output)?;

        channel.wait_close()?;
        let exit_status = channel.exit_status()?;

        session.disconnect(None, "Done", None)?;
        Ok((output, exit_status))
    }

    pub fn capture(&self, command: &str) -> Result<(String, String)> {
        let session = self.connect()?;

        let mut channel = session.channel_session()?;
        channel.exec(command)?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        let mut stderr_channel = channel.stderr();
        stderr_channel.read_to_string(&mut stderr)?;
        channel.read_to_string(&mut stdout)?;

        channel.wait_close()?;
        let exit_status = channel.exit_status()?;

        session.disconnect(None, "Done", None)?;

        if exit_status != 0 && !stderr.is_empty() {
            anyhow::bail!("Command exited with status {}: {}", exit_status, stderr);
        }

        Ok((stdout, stderr))
    }

    pub fn capture_output(&self, command: &str) -> Result<CommandOutput> {
        let session = self.connect()?;

        let mut channel = session.channel_session()?;
        channel.exec(command)?;

        let mut stdout = Vec::new();
        channel.read_to_end(&mut stdout)?;

        let mut stderr_reader = channel.stderr();
        let mut stderr = Vec::new();
        stderr_reader.read_to_end(&mut stderr)?;

        channel.wait_close()?;
        let exit_status = channel.exit_status()?;

        session.disconnect(None, "Done", None)?;

        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
            exit_code: exit_status,
        })
    }

    pub fn test_connection(&self) -> Result<()> {
        let (_, stderr) = self.capture("echo 'SSH connection test'")?;
        if stderr.contains("connection refused")
            || stderr.contains("no route to host")
            || stderr.contains("connection timed out")
        {
            anyhow::bail!("Failed to connect to {}: {}", self.host, stderr);
        }
        Ok(())
    }

    pub fn check_tmux_available(&self) -> Result<bool> {
        let (output, _) = self.execute_with_exit_code("which tmux 2>/dev/null || echo ''")?;
        Ok(!output.trim().is_empty())
    }
}

pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

pub struct ConnectionPool {
    clients: HashMap<String, Arc<SshClient>>,
}

impl ConnectionPool {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    pub fn get_or_create(&mut self, host_config: &RemoteHost) -> Result<Arc<SshClient>> {
        let key = format!(
            "{}@{}:{}",
            host_config.user, host_config.host, host_config.port
        );
        if let Some(client) = self.clients.get(&key) {
            return Ok(client.clone());
        }
        let client = SshClient::from_config(host_config)?;
        let client = Arc::new(client);
        self.clients.insert(key, client.clone());
        Ok(client)
    }

    pub fn remove(&mut self, host: &str, user: &str, port: u16) {
        let key = format!("{}@{}:{}", user, host, port);
        self.clients.remove(&key);
    }

    pub fn clear(&mut self) {
        self.clients.clear();
    }
}

impl Default for ConnectionPool {
    fn default() -> Self {
        Self::new()
    }
}
