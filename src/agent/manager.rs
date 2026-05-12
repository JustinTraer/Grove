use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::{Agent, AgentStatus};
use crate::app::config::{AiAgent, RemoteHost};
use crate::git::Worktree;
use crate::ssh::remote::RemoteTmuxSession;
use crate::ssh::SshClient;
use crate::tmux::TmuxSession;

/// Manages the lifecycle of agents.
pub struct AgentManager {
    pub repo_path: String,
    pub worktree_base: PathBuf,
}

impl AgentManager {
    pub fn new(repo_path: &str, worktree_base: PathBuf) -> Self {
        Self {
            repo_path: repo_path.to_string(),
            worktree_base,
        }
    }

    /// Create a new agent with worktree and tmux session.
    pub fn create_agent(
        &self,
        name: &str,
        branch: &str,
        ai_agent: &AiAgent,
        worktree_symlinks: &[String],
    ) -> Result<Agent> {
        tracing::debug!(
            "AgentManager::create_agent - name: {:?}, branch: {:?}, repo_path: {:?}, worktree_base: {:?}",
            name,
            branch,
            self.repo_path,
            self.worktree_base
        );
        let worktree = Worktree::new(&self.repo_path, self.worktree_base.clone());
        let worktree_path = worktree
            .create(branch)
            .context("Failed to create worktree")?;

        worktree
            .create_symlinks(&worktree_path, worktree_symlinks)
            .context("Failed to create worktree symlinks")?;

        let agent = Agent::new(name.to_string(), branch.to_string(), worktree_path.clone());

        let session = TmuxSession::new(&agent.tmux_session);
        session
            .create(&worktree_path, ai_agent.command())
            .context("Failed to create tmux session")?;

        Ok(agent)
    }

    /// Create a new remote agent with SSH worktree and remote tmux session.
    pub fn create_remote_agent(
        &self,
        name: &str,
        branch: &str,
        ai_agent: &AiAgent,
        remote_host: &RemoteHost,
    ) -> Result<Agent> {
        tracing::debug!(
            "AgentManager::create_remote_agent - name: {:?}, branch: {:?}, host: {:?}",
            name,
            branch,
            remote_host.host
        );

        let ssh_client = SshClient::from_config(remote_host)?;
        let worktree_id = uuid::Uuid::new_v4().to_string();
        let remote_worktree_path = remote_host.worktree_path(&worktree_id);

        tracing::debug!("Creating remote worktree at: {}", remote_worktree_path);

        ssh_client.execute(&format!(
            "mkdir -p {} && cd {} && git worktree add {} {} 2>/dev/null || git clone --bare . {} && cd {} && git checkout -b {}",
            remote_worktree_path,
            remote_worktree_path,
            branch,
            branch,
            remote_worktree_path,
            remote_worktree_path,
            branch
        ))?;

        let mut agent = Agent::new(
            name.to_string(),
            branch.to_string(),
            remote_worktree_path.clone(),
        );
        agent.remote_host = Some(remote_host.name.clone());
        agent.remote_worktree_path = Some(remote_worktree_path.clone());

        let remote_session = RemoteTmuxSession::new(&ssh_client, &agent.tmux_session);
        remote_session.create(&remote_worktree_path, ai_agent.command())?;

        Ok(agent)
    }

    /// Delete an agent, cleaning up worktree and tmux session.
    pub fn delete_agent(&self, agent: &Agent) -> Result<()> {
        if agent.is_remote() {
            self.delete_remote_agent(agent)?;
        } else {
            self.delete_local_agent(agent)?;
        }
        Ok(())
    }

    fn delete_local_agent(&self, agent: &Agent) -> Result<()> {
        let session = TmuxSession::new(&agent.tmux_session);
        if session.exists() {
            session.kill().context("Failed to kill tmux session")?;
        }

        if Path::new(&agent.worktree_path).exists() {
            let worktree = Worktree::new(&self.repo_path, self.worktree_base.clone());
            worktree
                .remove(&agent.worktree_path)
                .context("Failed to remove worktree")?;
        }

        Ok(())
    }

    fn delete_remote_agent(&self, agent: &Agent) -> Result<()> {
        let remote_host_name = agent
            .remote_host
            .as_ref()
            .context("Remote agent missing host configuration")?;
        let worktree_path = agent
            .remote_worktree_path
            .as_ref()
            .context("Remote agent missing worktree path")?;

        let config = crate::app::config::Config::load()?;
        let host_config = config
            .global
            .ssh
            .get_host(remote_host_name)
            .context(format!("Unknown remote host: {}", remote_host_name))?;

        let ssh_client = SshClient::from_config(host_config)?;
        let remote_session = RemoteTmuxSession::new(&ssh_client, &agent.tmux_session);

        if remote_session.exists() {
            remote_session.kill()?;
        }

        ssh_client.execute(&format!("rm -rf {}", worktree_path))?;

        Ok(())
    }

    /// Attach to an agent's tmux session.
    /// Auto-recreates the session if it doesn't exist (e.g., after system restart).
    pub fn attach_to_agent(&self, agent: &Agent, ai_agent: &AiAgent) -> Result<()> {
        if agent.is_remote() {
            self.attach_remote_agent(agent, ai_agent)
        } else {
            self.attach_local_agent(agent, ai_agent)
        }
    }

    fn attach_local_agent(&self, agent: &Agent, ai_agent: &AiAgent) -> Result<()> {
        let session = TmuxSession::new(&agent.tmux_session);

        if !session.exists() {
            session
                .create(&agent.worktree_path, ai_agent.command())
                .context("Failed to create tmux session")?;
        }

        session.attach()
    }

    fn attach_remote_agent(&self, agent: &Agent, ai_agent: &AiAgent) -> Result<()> {
        let remote_host_name = agent
            .remote_host
            .as_ref()
            .context("Remote agent missing host configuration")?;

        let config = crate::app::config::Config::load()?;
        let host_config = config
            .global
            .ssh
            .get_host(remote_host_name)
            .context(format!("Unknown remote host: {}", remote_host_name))?;

        let ssh_client = SshClient::from_config(host_config)?;
        let remote_session = RemoteTmuxSession::new(&ssh_client, &agent.tmux_session);

        if !remote_session.exists() {
            let worktree_path = agent
                .remote_worktree_path
                .as_ref()
                .context("Remote agent missing worktree path")?;
            remote_session.create(worktree_path, ai_agent.command())?;
        }

        remote_session.attach()
    }

    /// Get the current output from an agent's tmux session.
    pub fn capture_output(&self, agent: &Agent, lines: usize) -> Result<String> {
        if agent.is_remote() {
            self.capture_remote_output(agent, lines)
        } else {
            self.capture_local_output(agent, lines)
        }
    }

    fn capture_local_output(&self, agent: &Agent, lines: usize) -> Result<String> {
        let session = TmuxSession::new(&agent.tmux_session);

        if !session.exists() {
            return Ok(String::new());
        }

        session.capture_pane(lines)
    }

    fn capture_remote_output(&self, agent: &Agent, lines: usize) -> Result<String> {
        let remote_host_name = agent
            .remote_host
            .as_ref()
            .context("Remote agent missing host configuration")?;

        let config = crate::app::config::Config::load()?;
        let host_config = config
            .global
            .ssh
            .get_host(remote_host_name)
            .context(format!("Unknown remote host: {}", remote_host_name))?;

        let ssh_client = SshClient::from_config(host_config)?;
        let remote_session = RemoteTmuxSession::new(&ssh_client, &agent.tmux_session);

        if !remote_session.exists() {
            return Ok(String::new());
        }

        remote_session.capture_pane(lines)
    }

    /// Detect the current status of an agent from its output.
    pub fn detect_status(&self, agent: &Agent) -> Result<AgentStatus> {
        let output = self.capture_output(agent, 50)?;
        Ok(super::detector::detect_status(&output).status)
    }

    /// Send input to an agent's tmux session.
    pub fn send_input(&self, agent: &Agent, input: &str) -> Result<()> {
        if agent.is_remote() {
            self.send_remote_input(agent, input)
        } else {
            self.send_local_input(agent, input)
        }
    }

    fn send_local_input(&self, agent: &Agent, input: &str) -> Result<()> {
        let session = TmuxSession::new(&agent.tmux_session);

        if !session.exists() {
            anyhow::bail!("Tmux session does not exist");
        }

        session.send_keys(input)
    }

    fn send_remote_input(&self, agent: &Agent, input: &str) -> Result<()> {
        let remote_host_name = agent
            .remote_host
            .as_ref()
            .context("Remote agent missing host configuration")?;

        let config = crate::app::config::Config::load()?;
        let host_config = config
            .global
            .ssh
            .get_host(remote_host_name)
            .context(format!("Unknown remote host: {}", remote_host_name))?;

        let ssh_client = SshClient::from_config(host_config)?;
        let remote_session = RemoteTmuxSession::new(&ssh_client, &agent.tmux_session);

        if !remote_session.exists() {
            anyhow::bail!("Remote tmux session does not exist");
        }

        remote_session.send_keys(input)
    }

    /// Check if an agent's tmux session is still alive.
    pub fn is_session_alive(&self, agent: &Agent) -> bool {
        if agent.is_remote() {
            self.is_remote_session_alive(agent)
        } else {
            self.is_local_session_alive(agent)
        }
    }

    fn is_local_session_alive(&self, agent: &Agent) -> bool {
        let session = TmuxSession::new(&agent.tmux_session);
        session.exists()
    }

    fn is_remote_session_alive(&self, agent: &Agent) -> bool {
        let remote_host_name = match agent.remote_host.as_ref() {
            Some(name) => name,
            None => return false,
        };

        let config = match crate::app::config::Config::load() {
            Ok(c) => c,
            Err(_) => return false,
        };

        let host_config = match config.global.ssh.get_host(remote_host_name) {
            Some(h) => h,
            None => return false,
        };

        let ssh_client = match SshClient::from_config(host_config) {
            Ok(c) => c,
            Err(_) => return false,
        };

        let remote_session = RemoteTmuxSession::new(&ssh_client, &agent.tmux_session);
        remote_session.exists()
    }

    /// Restart an agent's AI session.
    pub fn restart_agent(&self, agent: &Agent, ai_agent: &AiAgent) -> Result<()> {
        if agent.is_remote() {
            self.restart_remote_agent(agent, ai_agent)
        } else {
            self.restart_local_agent(agent, ai_agent)
        }
    }

    fn restart_local_agent(&self, agent: &Agent, ai_agent: &AiAgent) -> Result<()> {
        let session = TmuxSession::new(&agent.tmux_session);

        if !session.exists() {
            session.create(&agent.worktree_path, ai_agent.command())?;
        } else {
            let _ = std::process::Command::new("tmux")
                .args(["send-keys", "-t", &agent.tmux_session, "C-c"])
                .output();

            std::thread::sleep(std::time::Duration::from_millis(100));
            session.send_keys(ai_agent.command())?;
        }

        Ok(())
    }

    fn restart_remote_agent(&self, agent: &Agent, ai_agent: &AiAgent) -> Result<()> {
        let remote_host_name = agent
            .remote_host
            .as_ref()
            .context("Remote agent missing host configuration")?;

        let config = crate::app::config::Config::load()?;
        let host_config = config
            .global
            .ssh
            .get_host(remote_host_name)
            .context(format!("Unknown remote host: {}", remote_host_name))?;

        let ssh_client = SshClient::from_config(host_config)?;
        let remote_session = RemoteTmuxSession::new(&ssh_client, &agent.tmux_session);
        let worktree_path = agent
            .remote_worktree_path
            .as_ref()
            .context("Remote agent missing worktree path")?;

        if !remote_session.exists() {
            remote_session.create(worktree_path, ai_agent.command())?;
        } else {
            remote_session.send_keys_raw("C-c")?;
            std::thread::sleep(std::time::Duration::from_millis(100));
            remote_session.send_keys(ai_agent.command())?;
        }

        Ok(())
    }

    /// Get info about all currently running grove sessions.
    pub fn list_running_sessions() -> Result<Vec<String>> {
        crate::tmux::list_grove_sessions()
    }

    /// Recover orphaned sessions (sessions without agents).
    pub fn find_orphaned_sessions(&self, known_agents: &[Uuid]) -> Result<Vec<String>> {
        let sessions = Self::list_running_sessions()?;
        let known_session_names: Vec<String> = known_agents
            .iter()
            .map(|id| format!("grove-{}", id.as_simple()))
            .collect();

        Ok(sessions
            .into_iter()
            .filter(|s| !known_session_names.contains(s))
            .collect())
    }
}
