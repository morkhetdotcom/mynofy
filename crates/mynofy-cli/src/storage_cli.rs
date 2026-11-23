//! Offline storage maintenance. Both owner stores stay locked throughout the operation.
use super::*;
use mynofy_server::content::offline::OfflineStorage;
pub async fn run(args: Vec<String>) -> Result<(), BoxError> {
    if args.is_empty() || args == ["help"] || args == ["--help"] {
        println!("{HELP}");
        return Ok(());
    }
    let mut args = args.into_iter();
    let operation = args
        .next()
        .ok_or("Expected storage check|inspect|cleanup|backup|restore|rekey|topic-ram")?;
    let mut flags = BTreeMap::new();
    while let Some(flag) = args.next() {
        if !flag.starts_with("--") || flags.contains_key(&flag) {
            return Err("Expected unique storage flags".into());
        }
        flags.insert(flag, args.next().ok_or("Missing storage flag value")?);
    }
    let allowed: &[&str] = match operation.as_str() {
        "check" | "inspect" | "cleanup" | "rekey" => &["--config"],
        "backup" => &["--config", "--out"],
        "restore" => &["--config", "--from", "--backup-secret"],
        "topic-ram" => &["--config", "--topic"],
        _ => {
            return Err(
                "Expected storage check|inspect|cleanup|backup|restore|rekey|topic-ram".into(),
            );
        }
    };
    if flags.keys().any(|flag| !allowed.contains(&flag.as_str())) {
        return Err("Unknown storage flag".into());
    }
    let required = |name: &str| {
        flags
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| BoxError::from(format!("Missing {name}")))
    };
    // Validate the complete command before stdin prompts or any storage access.
    match operation.as_str() {
        "backup" => {
            required("--out")?;
        }
        "restore" => {
            required("--from")?;
            if flags
                .get("--backup-secret")
                .is_some_and(|value| value != "stdin")
            {
                return Err("--backup-secret must be stdin".into());
            }
        }
        "topic-ram" => {
            required("--topic")?;
        }
        _ => (),
    }
    let config = Config::load(Path::new(required("--config")?))?;
    let secret = content_secret::read(&config).await?;
    let mut storage = if operation == "restore" {
        OfflineStorage::open_for_restore(config.clone(), secret)?
    } else {
        OfflineStorage::open(config.clone(), secret)?
    };
    match operation.as_str() {
        "check" | "inspect" => (),
        "cleanup" => storage.cleanup()?,
        "backup" => storage.backup(Path::new(required("--out")?))?,
        "restore" => {
            let backup_secret = if let Some(value) = flags.get("--backup-secret") {
                if value != "stdin" {
                    return Err("--backup-secret must be stdin".into());
                }
                content_secret::read_label(&config, "Backup content").await?
            } else {
                None
            };
            storage.restore_with_secret(Path::new(required("--from")?), backup_secret)?;
        }
        "topic-ram" => storage.topic_to_ram(required("--topic")?)?,
        "rekey" => {
            let secret = content_secret::read_label(&config, "New content")
                .await?
                .ok_or("Rekey requires encryption")?;
            let confirmation = content_secret::read_label(&config, "Confirm new content")
                .await?
                .ok_or("Missing replacement secret confirmation")?;
            let matches = match (&secret, &confirmation) {
                (
                    mynofy_server::content::ContentSecret::Password(a),
                    mynofy_server::content::ContentSecret::Password(b),
                ) => a.as_slice() == b.as_slice(),
                (
                    mynofy_server::content::ContentSecret::Key(a),
                    mynofy_server::content::ContentSecret::Key(b),
                ) => a.as_ref() == b.as_ref(),
                _ => false,
            };
            if !matches {
                return Err("Replacement secrets do not match; storage unchanged".into());
            }
            storage.rekey(secret)?;
        }
        _ => unreachable!(),
    }
    println!(
        "{}",
        if operation == "inspect" {
            storage.inspect()
        } else {
            storage.check()
        }
    );
    Ok(())
}

const HELP: &str = "mynofy storage check --config FILE
mynofy storage inspect --config FILE
mynofy storage cleanup --config FILE
mynofy storage backup --config FILE --out NEW_DIRECTORY
mynofy storage restore --config FILE --from BACKUP_DIRECTORY [--backup-secret stdin]
mynofy storage rekey --config FILE
mynofy storage topic-ram --config FILE --topic TOPIC

Offline only: stop the service first. Content and Security locks remain held.
Secrets are read from stdin. Rekey reads current, new, then confirmation.
Inspect prints metadata without message contents. Cleanup removes only unreferenced
internal slots and staging files after full validation. Security is never restored
from a content backup. Topic-to-RAM requires a RAM baseline in the configuration.";
