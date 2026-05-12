use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use super::SshClient;

pub struct RemoteTmuxSession<'a> {
    client: &'a SshClient,
    session_name: String,
    pane_id: Option<String>,
}

impl<'a> RemoteTmuxSession<'a> {
    pub fn new(client: &'a SshClient, session_name: &str) -> Self {
        Self {
            client,
            session_name: session_name.to_string(),
            pane_id: None,
        }
    }

    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    pub fn pane_id(&self) -> Option<&str> {
        self.pane_id.as_deref()
    }

    pub fn create(&self, working_dir: &str, command: &str) -> Result<()> {
        let escaped_dir = self.escape_shell_arg(working_dir);
        let escaped_cmd = self.escape_shell_arg(command);

        let tmux_cmd = format!(
            "tmux new-session -d -s '{}' -c {} && tmux send-keys -t '{}' {} Enter",
            self.session_name, escaped_dir, self.session_name, escaped_cmd
        );

        self.client.execute(&tmux_cmd)?;
        Ok(())
    }

    pub fn create_with_dimensions(
        &self,
        working_dir: &str,
        command: &str,
        width: u16,
        height: u16,
    ) -> Result<()> {
        let escaped_dir = self.escape_shell_arg(working_dir);
        let escaped_cmd = self.escape_shell_arg(command);

        let tmux_cmd = format!(
            "tmux new-session -d -s '{}' -x {} -y {} -c {} && tmux send-keys -t '{}' {} Enter",
            self.session_name, width, height, escaped_dir, self.session_name, escaped_cmd
        );

        self.client.execute(&tmux_cmd)?;
        Ok(())
    }

    pub fn exists(&self) -> bool {
        self.client
            .execute(&format!(
                "tmux has-session -t '{}' 2>/dev/null",
                self.session_name
            ))
            .is_ok()
    }

    pub fn capture_pane(&self, lines: usize) -> Result<String> {
        let output = self.client.capture_output(&format!(
            "tmux capture-pane -t '{}' -p -e -J -S -{}",
            self.session_name, lines
        ))?;

        if !output.success()
            && !output.stderr.is_empty()
            && !output.stderr.contains("no current window")
        {
            anyhow::bail!("Failed to capture pane: {}", output.stderr);
        }

        Ok(output.stdout)
    }

    pub fn send_keys_raw(&self, keys: &str) -> Result<()> {
        let escaped_keys = self.escape_shell_arg(keys);
        self.client.execute(&format!(
            "tmux send-keys -t '{}' -l {}",
            self.session_name, escaped_keys
        ))?;
        Ok(())
    }

    pub fn send_keys(&self, keys: &str) -> Result<()> {
        let escaped_keys = self.escape_shell_arg(keys);
        self.client.execute(&format!(
            "tmux send-keys -t '{}' {} && tmux send-keys -t '{}' Enter",
            self.session_name, escaped_keys, self.session_name
        ))?;
        Ok(())
    }

    pub fn send_text(&self, text: &str) -> Result<()> {
        let escaped_text = self.escape_shell_arg(text);
        self.client.execute(&format!(
            "tmux set-buffer -t '{}' {} && tmux paste-buffer -t '{}'",
            self.session_name, escaped_text, self.session_name
        ))?;
        Ok(())
    }

    pub fn kill(&self) -> Result<()> {
        self.client.execute(&format!(
            "tmux kill-session -t '{}' 2>/dev/null || true",
            self.session_name
        ))?;
        Ok(())
    }

    pub fn pane_current_command(&self) -> Option<String> {
        self.client
            .execute(&format!(
                "tmux display-message -t '{}' -p '#{{pane_current_command}}'",
                self.session_name
            ))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn pane_size(&self) -> Result<(u16, u16)> {
        let output = self.client.execute(&format!(
            "tmux display-message -t '{}' -p '#{{pane_width}} #{{pane_height}}'",
            self.session_name
        ))?;

        let parts: Vec<&str> = output.split_whitespace().collect();
        if parts.len() >= 2 {
            let width = parts[0].parse().unwrap_or(80);
            let height = parts[1].parse().unwrap_or(24);
            Ok((width, height))
        } else {
            Ok((80, 24))
        }
    }

    pub fn is_attached(&self) -> bool {
        self.client
            .execute(&format!(
                "tmux list-clients -t '{}' 2>/dev/null | grep -q .",
                self.session_name
            ))
            .is_ok()
    }

    pub fn attach_command(&self) -> String {
        format!("tmux -CC attach-session -t '{}'", self.session_name)
    }

    pub fn local_attach_command(&self) -> String {
        format!(
            "ssh -t {}@{} -p {} '{}'",
            self.client.user(),
            self.client.host(),
            self.client.port(),
            self.attach_command()
        )
    }

    pub fn attach(&self) -> Result<()> {
        let local_cmd = self.local_attach_command();
        let status = Command::new("sh")
            .args(["-c", &local_cmd])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("Failed to run tmux attach")?;

        if !status.success() {
            anyhow::bail!("tmux attach exited with error");
        }

        Ok(())
    }

    fn escape_shell_arg(&self, arg: &str) -> String {
        let escaped = arg.replace("'", "'\\''");
        format!("'{}'", escaped)
    }
}

pub fn remote_list_sessions(client: &SshClient) -> Result<Vec<String>> {
    let output = client.execute("tmux list-sessions -F '#{session_name}' 2>/dev/null || true")?;
    Ok(output
        .lines()
        .filter(|s| s.starts_with("grove-"))
        .map(String::from)
        .collect())
}

pub fn is_tmux_available_on_remote(client: &SshClient) -> Result<bool> {
    let (output, exit_code) =
        client.execute_with_exit_code("tmux -V 2>/dev/null || echo 'not found'")?;
    Ok(exit_code == 0 && !output.trim().is_empty())
}
