use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::info;

use crate::config::ProvisionConfig;

/// A provisioned database instance ready for replay.
#[derive(Debug)]
pub struct ProvisionedDb {
    pub connection_string: String,
    pub container_id: Option<String>,
}

/// Provision a database based on config.
pub fn provision(config: &ProvisionConfig) -> Result<ProvisionedDb> {
    if let Some(conn) = &config.connection_string {
        info!("Using pre-existing connection: {}", conn);
        return Ok(ProvisionedDb {
            connection_string: conn.clone(),
            container_id: None,
        });
    }

    match config.backend.as_str() {
        "docker" => provision_docker(config),
        other => anyhow::bail!("Unknown provision backend: {other}. Supported: docker"),
    }
}

/// Tear down a provisioned database.
pub fn teardown(db: &ProvisionedDb) -> Result<()> {
    if let Some(id) = &db.container_id {
        info!("Stopping container {}", &id[..12.min(id.len())]);
        let output = Command::new("docker")
            .args(["rm", "-f", id])
            .output()
            .context("Failed to run docker rm")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker rm failed: {stderr}");
        }
    }
    Ok(())
}

fn provision_docker(config: &ProvisionConfig) -> Result<ProvisionedDb> {
    let image = config.image.as_deref().unwrap_or("postgres:16");

    // Check docker is available
    Command::new("docker")
        .arg("version")
        .output()
        .context("Docker not available. Is Docker installed and running?")?;

    let port = config.port.unwrap_or(0);
    let host_port = if port == 0 {
        "0".to_string()
    } else {
        port.to_string()
    };

    info!("Starting Docker container from {image}...");

    let output = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &format!("pg-retest-{}", std::process::id()),
            "-e",
            "POSTGRES_USER=pgretest",
            "-e",
            "POSTGRES_PASSWORD=pgretest",
            "-e",
            "POSTGRES_DB=pgretest",
            "-p",
            &format!("{host_port}:5432"),
            image,
        ])
        .output()
        .context("Failed to start Docker container")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("docker run failed: {stderr}");
    }

    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    info!(
        "Container started: {}",
        &container_id[..12.min(container_id.len())]
    );

    let mapped_port = get_mapped_port(&container_id)?;
    info!("PostgreSQL available on port {mapped_port}");

    wait_for_pg(&container_id)?;

    if let Some(restore_path) = &config.restore_from {
        restore_backup(&container_id, restore_path)?;
    }

    let connection_string = format!(
        "host=127.0.0.1 port={mapped_port} user=pgretest password=pgretest dbname=pgretest"
    );

    Ok(ProvisionedDb {
        connection_string,
        container_id: Some(container_id),
    })
}

fn get_mapped_port(container_id: &str) -> Result<u16> {
    let output = Command::new("docker")
        .args(["port", container_id, "5432"])
        .output()
        .context("Failed to get container port")?;

    let port_str = String::from_utf8_lossy(&output.stdout);
    let port = port_str
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .context("Failed to parse mapped port")?;

    Ok(port)
}

fn wait_for_pg(container_id: &str) -> Result<()> {
    info!("Waiting for PostgreSQL to be ready...");
    for attempt in 1..=30 {
        let output = Command::new("docker")
            .args(["exec", container_id, "pg_isready", "-U", "pgretest"])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                info!("PostgreSQL ready (attempt {attempt})");
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    anyhow::bail!("PostgreSQL did not become ready within 30 seconds")
}

fn restore_backup(container_id: &str, path: &Path) -> Result<()> {
    info!("Restoring backup from {}", path.display());

    let container_path = "/tmp/restore.sql";
    let output = Command::new("docker")
        .args([
            "cp",
            &path.to_string_lossy(),
            &format!("{container_id}:{container_path}"),
        ])
        .output()
        .context("Failed to copy backup to container")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("docker cp failed: {stderr}");
    }

    let output = Command::new("docker")
        .args([
            "exec",
            container_id,
            "psql",
            "-U",
            "pgretest",
            "-d",
            "pgretest",
            "-f",
            container_path,
        ])
        .output()
        .context("Failed to restore backup")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("FATAL") || stderr.contains("could not") {
            anyhow::bail!("Backup restore failed: {stderr}");
        }
    }

    info!("Backup restored successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provision_with_connection_string() {
        let config = ProvisionConfig {
            backend: "docker".into(),
            image: None,
            restore_from: None,
            connection_string: Some("host=localhost dbname=test".into()),
            port: None,
        };
        let db = provision(&config).unwrap();
        assert_eq!(db.connection_string, "host=localhost dbname=test");
        assert!(db.container_id.is_none());
    }

    #[test]
    fn test_provision_unknown_backend() {
        let config = ProvisionConfig {
            backend: "kubernetes".into(),
            image: None,
            restore_from: None,
            connection_string: None,
            port: None,
        };
        let err = provision(&config).unwrap_err();
        assert!(err.to_string().contains("Unknown provision backend"));
    }

    #[test]
    fn test_teardown_no_container() {
        let db = ProvisionedDb {
            connection_string: "host=localhost".into(),
            container_id: None,
        };
        teardown(&db).unwrap(); // should be a no-op
    }
}
