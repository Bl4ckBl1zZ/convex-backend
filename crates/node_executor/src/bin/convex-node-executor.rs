use std::{
    env,
    fs,
    path::PathBuf,
    process::Command,
};

use anyhow::Context;
use isolate::bundled_js::node_executor_file;

const DEFAULT_PORT: u16 = 3002;
const MIN_SHARED_SECRET_BYTES: usize = 32;

fn main() -> anyhow::Result<()> {
    let shared_secret = env::var("NODE_EXECUTOR_SHARED_SECRET")
        .context("NODE_EXECUTOR_SHARED_SECRET is required for the remote Node executor")?;
    anyhow::ensure!(
        shared_secret.len() >= MIN_SHARED_SECRET_BYTES,
        "NODE_EXECUTOR_SHARED_SECRET must be at least {MIN_SHARED_SECRET_BYTES} bytes"
    );

    let port = match env::var("NODE_EXECUTOR_PORT") {
        Ok(port) => port
            .parse::<u16>()
            .context("NODE_EXECUTOR_PORT must be a valid TCP port")?,
        Err(env::VarError::NotPresent) => DEFAULT_PORT,
        Err(env::VarError::NotUnicode(_)) => {
            anyhow::bail!("NODE_EXECUTOR_PORT must contain valid UTF-8")
        },
    };
    let root_dir = env::var_os("NODE_EXECUTOR_TEMP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/convex-node-executor"));
    let launcher_dir = root_dir.join("launcher");
    let work_dir = root_dir.join("work");
    fs::create_dir_all(&launcher_dir)?;
    fs::create_dir_all(&work_dir)?;

    let (source, source_map) =
        node_executor_file("local.cjs").context("local.cjs is not embedded in this binary")?;
    fs::write(launcher_dir.join("local.cjs"), source.as_bytes())?;
    if let Some(source_map) = source_map {
        fs::write(launcher_dir.join("local.cjs.map"), source_map.as_bytes())?;
    }

    let mut command = Command::new("node");
    command
        .arg(launcher_dir.join("local.cjs"))
        .arg("--port")
        .arg(port.to_string())
        .arg("--tempdir")
        .arg(work_dir)
        .env("NODE_EXECUTOR_REQUIRE_AUTH", "true");
    if env::var("NODE_EXECUTOR_DEBUG").is_ok() {
        command.arg("--debug");
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec().into())
    }

    #[cfg(not(unix))]
    {
        let status = command.status()?;
        anyhow::ensure!(status.success(), "Node executor exited with {status}");
        Ok(())
    }
}
