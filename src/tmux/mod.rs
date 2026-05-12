pub mod session;

pub use session::{is_tmux_available, list_grove_sessions, TmuxSession};

pub enum TmuxHandle<'a> {
    Local(TmuxSession),
    Remote(crate::ssh::remote::RemoteTmuxSession<'a>),
}

impl TmuxHandle<'_> {
    pub fn exists(&self) -> bool {
        match self {
            TmuxHandle::Local(s) => s.exists(),
            TmuxHandle::Remote(s) => s.exists(),
        }
    }

    pub fn create(&self, working_dir: &str, command: &str) -> anyhow::Result<()> {
        match self {
            TmuxHandle::Local(s) => s.create(working_dir, command),
            TmuxHandle::Remote(s) => s.create(working_dir, command),
        }
    }

    pub fn capture_pane(&self, lines: usize) -> anyhow::Result<String> {
        match self {
            TmuxHandle::Local(s) => s.capture_pane(lines),
            TmuxHandle::Remote(s) => s.capture_pane(lines),
        }
    }

    pub fn send_keys(&self, keys: &str) -> anyhow::Result<()> {
        match self {
            TmuxHandle::Local(s) => s.send_keys(keys),
            TmuxHandle::Remote(s) => s.send_keys(keys),
        }
    }

    pub fn kill(&self) -> anyhow::Result<()> {
        match self {
            TmuxHandle::Local(s) => s.kill(),
            TmuxHandle::Remote(s) => s.kill(),
        }
    }

    pub fn pane_current_command(&self) -> Option<String> {
        match self {
            TmuxHandle::Local(s) => s.pane_current_command(),
            TmuxHandle::Remote(s) => s.pane_current_command(),
        }
    }

    pub fn pane_size(&self) -> anyhow::Result<(u16, u16)> {
        match self {
            TmuxHandle::Local(s) => s.pane_size(),
            TmuxHandle::Remote(s) => s.pane_size(),
        }
    }

    pub fn attach(&self) -> anyhow::Result<()> {
        match self {
            TmuxHandle::Local(s) => s.attach(),
            TmuxHandle::Remote(s) => s.attach(),
        }
    }
}
