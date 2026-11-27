use std::path::{Path, PathBuf};

use jiff::civil::Date;
use log::*;
use tokio::fs;

use crate::{
    file::{ensure_path_within_root, norm_abs_path},
    player::{
        controller::ChannelManager,
        utils::{JsonPlaylist, json_reader, json_serializer::daily_playlist_path, json_writer},
    },
    utils::{config::PlayoutConfig, errors::ServiceError, generator::playlist_generator},
};

pub(crate) fn playlist_path(config: &PlayoutConfig, date: &str) -> Result<PathBuf, ServiceError> {
    let invalid_date =
        || ServiceError::BadRequest("Invalid playlist date; expected YYYY-MM-DD".into());
    let parsed = Date::strptime("%Y-%m-%d", date).map_err(|_| invalid_date())?;
    let normalized = parsed.strftime("%Y-%m-%d").to_string();

    if date.len() != 10 || date != normalized {
        return Err(invalid_date());
    }

    let root = &config.channel.playlists;

    // A configured single-file playlist is an explicit trusted target.
    if root
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        return Ok(root.clone());
    }

    let path = root
        .join(parsed.strftime("%Y").to_string())
        .join(parsed.strftime("%m").to_string())
        .join(normalized)
        .with_extension("json");

    // Resolve existing ancestors too, so new files through directory links
    // cannot escape the root. Keep the lexical path: deletion must unlink a
    // day alias, rather than delete its shared source file. This preflight
    // check assumes links are not concurrently swapped by filesystem writers.
    ensure_path_within_root(root, &path)?;

    Ok(path)
}

/// Names of the entries in `dir` accepted by `keep`, newest (highest) first.
async fn sorted_entry_names(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(mut entries) = fs::read_dir(dir).await else {
        return names;
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Some(name) = entry.file_name().to_str()
            && keep(name)
        {
            names.push(name.to_string());
        }
    }

    names.sort_unstable_by(|a, b| b.cmp(a));

    names
}

fn is_number_with_len(name: &str, len: usize) -> bool {
    name.len() == len && name.bytes().all(|b| b.is_ascii_digit())
}

/// Date of the most recent daily playlist stored on or before `newest`.
///
/// An infinite playlist is not bound to a broadcast day: it keeps looping
/// across day, month and year changes. The file it was saved under therefore
/// usually carries an older date than the current one, and both the playout
/// and the editor have to find it from today's date.
///
/// Dates are compared as `YYYY-MM-DD` strings, which sort chronologically.
pub(crate) async fn latest_playlist_date(root: &Path, newest: &str) -> Option<String> {
    let (newest_year, newest_month) = (newest.get(..4)?, newest.get(..7)?);

    if fs::try_exists(daily_playlist_path(root, newest))
        .await
        .unwrap_or(false)
    {
        return Some(newest.to_string());
    }

    let years = sorted_entry_names(root, |name| {
        is_number_with_len(name, 4) && name <= newest_year
    })
    .await;

    for year in years {
        let year_dir = root.join(&year);
        let months = sorted_entry_names(&year_dir, |name| {
            is_number_with_len(name, 2) && format!("{year}-{name}").as_str() <= newest_month
        })
        .await;

        for month in months {
            let prefix = format!("{year}-{month}-");
            let dates = sorted_entry_names(&year_dir.join(&month), |name| {
                name.strip_suffix(".json").is_some_and(|date| {
                    date.starts_with(&prefix)
                        && date <= newest
                        && Date::strptime("%Y-%m-%d", date).is_ok()
                })
            })
            .await;

            if let Some(file_name) = dates.first() {
                return file_name.strip_suffix(".json").map(str::to_string);
            }
        }
    }

    None
}

/// Read the playlist an operator edits for `date`.
///
/// An infinite playlist has no file per day, so the most recent one on or
/// before `date` is returned. Its `date` field names the file it was read
/// from, which is where changes have to be saved to reach the playout.
pub async fn read_editable_playlist(
    config: &PlayoutConfig,
    date: String,
) -> Result<JsonPlaylist, ServiceError> {
    if !config.playlist.infinit {
        return read_playlist(config, date).await;
    }

    // Validate the requested date before it is compared with file names.
    playlist_path(config, &date)?;

    let Some(stored_date) = latest_playlist_date(&config.channel.playlists, &date).await else {
        return read_playlist(config, date).await;
    };
    let mut playlist = read_playlist(config, stored_date.clone()).await?;

    playlist.date = stored_date;

    Ok(playlist)
}

pub async fn read_playlist(
    config: &PlayoutConfig,
    date: String,
) -> Result<JsonPlaylist, ServiceError> {
    let playlist_path = playlist_path(config, &date)?;

    if !fs::try_exists(&playlist_path).await? {
        return Err(ServiceError::NoContent());
    }

    match json_reader(&playlist_path).await {
        Ok(p) => Ok(p),
        Err(e) => Err(ServiceError::Conflict(e.to_string())),
    }
}

pub async fn write_playlist(
    config: &PlayoutConfig,
    json_data: JsonPlaylist,
) -> Result<String, ServiceError> {
    let date = json_data.date.clone();
    let playlist_path = playlist_path(config, &date)?;

    let mut file_exists = false;

    if let Some(p) = playlist_path.parent() {
        fs::create_dir_all(p).await?;
    }

    if playlist_path.is_file() {
        file_exists = true;
        if let Ok(existing_data) = json_reader(&playlist_path).await
            && json_data == existing_data
        {
            return Err(ServiceError::Conflict(format!(
                "Playlist from {date}, already exists!"
            )));
        }
    }

    match json_writer(&playlist_path, json_data).await {
        Ok(..) => {
            return if file_exists {
                Ok(format!("Update playlist from {date} success!"))
            } else {
                Ok(format!("Write playlist from {date} success!"))
            };
        }
        Err(e) => {
            error!("{e}");
        }
    }

    Err(ServiceError::InternalServerError)
}

pub async fn generate_playlist(
    manager: ChannelManager,
    shuffle_override: Option<bool>,
) -> Result<JsonPlaylist, ServiceError> {
    let (template, storage_root) = {
        let mut config = manager.config.write().await;
        (
            config.general.template.take(),
            config.channel.storage.clone(),
        )
    };

    if let Some(mut template) = template {
        for source in &mut template.sources {
            let mut resolved_paths = Vec::with_capacity(source.paths.len());

            for path in &source.paths {
                let (safe_path, _, _) = norm_abs_path(&storage_root, &path.to_string_lossy())?;
                resolved_paths.push(safe_path);
            }

            source.paths = resolved_paths;
        }

        {
            let mut config = manager.config.write().await;
            config.general.template = Some(template);
        }
    }

    match playlist_generator(&manager, shuffle_override).await {
        Ok(playlists) => {
            if playlists.is_empty() {
                Err(ServiceError::Conflict(
                    "The playlist could not be written, maybe it already exists!".into(),
                ))
            } else {
                Ok(playlists[0].clone())
            }
        }
        Err(e) => {
            error!("{e}");
            Err(e)
        }
    }
}

pub async fn delete_playlist(config: &PlayoutConfig, date: &str) -> Result<String, ServiceError> {
    let playlist_path = playlist_path(config, date)?;

    if playlist_path == config.channel.playlists {
        return Err(ServiceError::BadRequest(
            "A fixed playlist file cannot be deleted by date".into(),
        ));
    }

    if playlist_path.is_file() || playlist_path.is_symlink() {
        match fs::remove_file(playlist_path).await {
            Ok(..) => Ok(format!("Delete playlist from {date} success!")),
            Err(e) => {
                error!("{e}");
                Err(ServiceError::InternalServerError)
            }
        }
    } else {
        Ok(format!("No playlist to delete on: {date}"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tokio::fs;

    use super::{latest_playlist_date, read_editable_playlist};
    use crate::{
        player::utils::json_serializer::daily_playlist_path, test_support::channel_manager,
        utils::errors::ServiceError,
    };

    async fn store_playlist(root: &Path, date: &str) {
        let path = daily_playlist_path(root, date);
        let contents = format!(
            r#"{{"date":"1999-01-01","program":[{{"source":"{date}.mp4","in":0.0,"out":30.0,"duration":30.0}}]}}"#
        );

        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(path, contents).await.unwrap();
    }

    #[tokio::test]
    async fn latest_playlist_is_found_across_day_month_and_year_changes() {
        let (manager, pool) = channel_manager().await;
        let config = manager.config.read().await.clone();
        let root = config.channel.playlists.clone();

        assert_eq!(latest_playlist_date(&root, "2026-11-30").await, None);

        store_playlist(&root, "2026-11-30").await;

        for today in ["2026-11-30", "2026-12-01", "2026-12-31", "2027-01-01"] {
            assert_eq!(
                latest_playlist_date(&root, today).await.as_deref(),
                Some("2026-11-30"),
                "{today}"
            );
        }

        assert_eq!(latest_playlist_date(&root, "2026-11-29").await, None);

        store_playlist(&root, "2026-12-15").await;
        store_playlist(&root, "2027-03-02").await;

        assert_eq!(
            latest_playlist_date(&root, "2026-12-14").await.as_deref(),
            Some("2026-11-30")
        );
        assert_eq!(
            latest_playlist_date(&root, "2027-01-01").await.as_deref(),
            Some("2026-12-15")
        );
        assert_eq!(
            latest_playlist_date(&root, "2027-03-02").await.as_deref(),
            Some("2027-03-02")
        );

        pool.close().await;
        fs::remove_dir_all(&config.channel.storage).await.unwrap();
    }

    #[tokio::test]
    async fn files_that_are_no_daily_playlist_are_ignored() {
        let (manager, pool) = channel_manager().await;
        let config = manager.config.read().await.clone();
        let root = config.channel.playlists.clone();
        let month = root.join("2026").join("11");

        fs::create_dir_all(&month).await.unwrap();
        fs::create_dir_all(root.join("backup")).await.unwrap();

        // A wrong folder, an impossible date, another extension and a copy.
        for name in [
            "2026-10-05.json",
            "2026-11-31.json",
            "2026-11-20.txt",
            "2026-11-20 copy.json",
        ] {
            fs::write(month.join(name), "{}").await.unwrap();
        }

        assert_eq!(latest_playlist_date(&root, "2026-12-01").await, None);

        pool.close().await;
        fs::remove_dir_all(&config.channel.storage).await.unwrap();
    }

    #[tokio::test]
    async fn editor_gets_the_running_infinite_playlist_after_the_day_changed() {
        let (manager, pool) = channel_manager().await;
        let mut config = manager.config.read().await.clone();
        let root = config.channel.playlists.clone();

        store_playlist(&root, "2026-11-30").await;

        config.playlist.infinit = false;

        assert!(matches!(
            read_editable_playlist(&config, "2026-12-01".into()).await,
            Err(ServiceError::NoContent())
        ));

        config.playlist.infinit = true;

        let playlist = read_editable_playlist(&config, "2026-12-01".into())
            .await
            .unwrap();

        // The date names the file to save to, not the one stored inside it.
        assert_eq!(playlist.date, "2026-11-30");
        assert_eq!(playlist.program[0].source, "2026-11-30.mp4");
        assert!(matches!(
            read_editable_playlist(&config, "../2026-12-01".into()).await,
            Err(ServiceError::BadRequest(_))
        ));

        pool.close().await;
        fs::remove_dir_all(&config.channel.storage).await.unwrap();
    }
}
