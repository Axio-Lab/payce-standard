use deadpool_postgres::Pool;
use std::fs;
use std::path::PathBuf;

pub async fn run_startup_migrations(pool: &Pool) -> Result<(), String> {
    let client = pool.get().await.map_err(|e| e.to_string())?;

    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                filename TEXT PRIMARY KEY,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )",
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut files: Vec<PathBuf> = fs::read_dir("migrations")
        .map_err(|e| format!("Could not read migrations directory: {e}"))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_lowercase() == "sql")
                    .unwrap_or(false)
        })
        .collect();

    files.sort();

    for path in files {
        let filename = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or("Invalid migration filename")?;

        let already_applied = client
            .query_opt(
                "SELECT filename FROM schema_migrations WHERE filename = $1",
                &[&filename],
            )
            .await
            .map_err(|e| e.to_string())?
            .is_some();

        if already_applied {
            continue;
        }

        if filename.starts_with("001_") {
            let users_exists = client
                .query_opt("SELECT to_regclass('public.users')::text", &[])
                .await
                .map_err(|e| e.to_string())?
                .and_then(|r| r.get::<_, Option<String>>(0))
                .is_some();

            if users_exists {
                client
                    .execute(
                        "INSERT INTO schema_migrations (filename) VALUES ($1) ON CONFLICT (filename) DO NOTHING",
                        &[&filename],
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                log::info!(
                    "[DB] Detected existing schema. Marked {} as applied.",
                    filename
                );
                continue;
            }
        }

        let sql = fs::read_to_string(&path)
            .map_err(|e| format!("Could not read migration {filename}: {e}"))?;

        if let Err(e) = client.batch_execute(&sql).await {
            let msg = e.to_string();
            if msg.contains("already exists") || msg.contains("duplicate key value") {
                log::warn!(
                    "[DB] Migration {} appears already applied ({}). Marking as applied.",
                    filename,
                    msg
                );
            } else {
                return Err(format!("Failed migration {filename}: {e}"));
            }
        }

        client
            .execute(
                "INSERT INTO schema_migrations (filename) VALUES ($1)",
                &[&filename],
            )
            .await
            .map_err(|e| e.to_string())?;

        log::info!("[DB] Applied migration: {}", filename);
    }

    Ok(())
}
