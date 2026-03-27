use std::path::PathBuf;

use clap::Parser;
use directories::ProjectDirs;

#[derive(Debug, Clone, Parser)]
#[command(name = "r15-shell", version, about = "Small Stack Overflow chat shell")]
pub struct Args {
    #[arg(long, env = "R15_ROOM_ID", default_value_t = 15)]
    pub room_id: u64,

    #[arg(long, env = "R15_POLL_SECONDS", default_value_t = 5)]
    pub poll_seconds: u64,

    #[arg(long, env = "R15_TRANSCRIPT_LIMIT", default_value_t = 40)]
    pub transcript_limit: usize,

    #[arg(long, env = "R15_COOKIE_FILE")]
    pub cookie_file: Option<PathBuf>,

    #[arg(long, env = "R15_CHAT_URL", default_value = "https://chat.stackoverflow.com")]
    pub chat_url: String,

    #[arg(long, env = "R15_SITE_URL", default_value = "https://stackoverflow.com")]
    pub site_url: String,
}

impl Args {
    pub fn cookie_file_path(&self) -> PathBuf {
        if let Some(path) = &self.cookie_file {
            return path.clone();
        }

        if let Some(project_dirs) = ProjectDirs::from("com", "rajaasim", "r15-shell") {
            return project_dirs.config_dir().join("cookie_header.txt");
        }

        PathBuf::from(".r15-shell").join("cookie_header.txt")
    }
}
