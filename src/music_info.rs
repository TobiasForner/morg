use core::time;
use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result, bail};
use distance::levenshtein;
use json::JsonValue;
use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::Album;

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Keys {
    pub key: String,
    pub secret: String,
}

impl Keys {
    fn keys_file() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("TF", "TF", "morg")
            .context("Failed to construct config path!")?;
        let keys_file = dirs.config_local_dir().join("keys.toml");
        Ok(keys_file)
    }
    pub fn parse() -> Result<Self> {
        let keys_file = Keys::keys_file()?;
        let text = std::fs::read_to_string(&keys_file)
            .context(format!(
                "Could not read {keys_file:?}. Does the file exist?"
            ))?
            .replace("\r\n", "\n");
        toml::from_str(&text).context("Could not parse keys from {keys_file:?}")
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AlbumInfo {
    pub artist: String,
    pub title: String,
    pub year: Option<i32>,
}

#[derive(Deserialize, Serialize)]
pub struct MusicInfoCache {
    cache: HashMap<String, AlbumInfo>,
    refresh: bool,
}
impl MusicInfoCache {
    pub fn new() -> Self {
        MusicInfoCache {
            cache: HashMap::new(),
            refresh: true,
        }
    }
    pub fn load(refresh: bool) -> Result<Self> {
        let dirs = directories::ProjectDirs::from("TF", "TF", "morg")
            .context("Failed to construct data path!")?;
        if !dirs.data_local_dir().exists() {
            std::fs::create_dir(dirs.data_local_dir())?;
        }
        let info_file = dirs.data_local_dir().join("music_info.toml");
        if info_file.exists() {
            let text = std::fs::read_to_string(&info_file)
                .context(format!(
                    "Could not read {info_file:?}. Does the file exist?"
                ))?
                .replace("\r\n", "\n");
            let mut res: MusicInfoCache =
                toml::from_str(&text).context("Could not parse music info from {info_file:?}")?;
            res.refresh = refresh;
            Ok(res)
        } else {
            Ok(MusicInfoCache::new())
        }
    }

    pub fn store(&self) -> Result<()> {
        let dirs = directories::ProjectDirs::from("TF", "TF", "morg")
            .context("Failed to construct data path!")?;
        let info_file = dirs.data_local_dir().join("music_info.toml");
        let text = toml::to_string(&self)?;
        std::fs::write(&info_file, text)?;
        Ok(())
    }

    pub fn get_album_info(&mut self, album: &Album) -> Result<AlbumInfo> {
        let key = album.key();
        if self.refresh || !self.cache.contains_key(&key) {
            let (album_info, limit) = get_album_info_discogs(album)?;
            self.cache.insert(key, album_info.clone());
            self.store().context("Failed to store cache")?;
            if limit <= 1 {
                println!("Waiting 60s to avoid rate limit...");

                std::thread::sleep(time::Duration::from_secs(60));
            }
            Ok(album_info)
        } else {
            self.cache.get(&key).context("not found in cache").cloned()
        }
    }
}

fn get_album_json(album: &Album) -> Result<(JsonValue, i32)> {
    let keys = Keys::parse()?;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = reqwest::Client::new();
    let url = "https://api.discogs.com/database/search";
    let params = [
        ("artist", album.artist.to_string()),
        ("album", album.title.to_string()),
        ("format", "album".to_string()),
        //("per_page", "30"),
        ("page", "5".to_string()),
        ("description", "Official Release".to_string()),
        ("key", keys.key.to_string()),
        ("secret", keys.secret.to_string()),
        (
            "user-agent",
            "morg: Music organizer, yamakantor@mnet-online.de".to_string(),
        ),
    ];
    let res = client
        .get(url)
        .header(
            USER_AGENT,
            "morg: Music organizer, yamakantor@mnet-online.de",
        )
        .query(&params)
        .send();
    let res = runtime.block_on(res);
    let res = res.unwrap();
    let headers = res.headers();
    let mut limit = 0;
    if let Some(rl) = headers.get("X-Discogs-Ratelimit-Remaining")
        && let Ok(rl) = rl.to_str()
    {
        limit = rl.parse().expect("rate limit should be a valid i32");
    }
    let content = runtime.block_on(res.text())?;
    let parsed = json::parse(&content)?;

    let search_title = format!("{} - {}", album.artist, album.title);
    parsed["results"]
        .clone()
        .members()
        .filter_map(|r| {
            if r.has_key("title") {
                let title = &r["title"].to_string();
                let score = levenshtein(&search_title, title);
                Some((r.clone(), score))
            } else {
                None
            }
        })
        .min_by_key(|(_, s)| *s)
        .map(|(r, _)| (r, limit))
        .context("")
}

pub fn download_cover_file(album: &mut Album) -> Result<i32> {
    let result = get_album_json(album);

    if let Ok((result, limit)) = result {
        if result.has_key("cover_image") {
            let cover_url = result["cover_image"]
                .as_str()
                .context("cover_image should be a valid str!")?;
            let ext = cover_url
                .rsplit_once(".")
                .context("Failed to determine cover file extension for {cover_url:?}")?;
            let cover_path = album.dir_path.join(format!("cover.{}", ext.1));
            println!("Downloading {cover_url} to {cover_path:?}");
            let mut file = std::fs::File::create(cover_path)?;
            reqwest::blocking::get(cover_url)?.copy_to(&mut file)?;
        }
        Ok(limit)
    } else {
        bail!(
            "Failed to find matching discogs result for {}",
            album.overview()
        );
    }
}

fn get_album_info_discogs(album: &Album) -> Result<(AlbumInfo, i32)> {
    let result = get_album_json(album);
    if let Ok((result, limit)) = result {
        let mut artist = None;
        let mut album_title = None;
        let title = result["title"].to_string();
        if let Some((aartist, atitle)) = title.split_once(" - ") {
            let mut aartist = aartist;
            (2..100).for_each(|i| {
                aartist = aartist.trim_end_matches(&format!(" ({i})"));
            });

            artist = Some(aartist);
            album_title = Some(atitle);
        }
        let mut year = None;
        if result.has_key("year")
            && let Some(ayear) = result["year"].as_str()
        {
            let r: Result<i32> = ayear.parse().context("");
            if let Ok(ayear) = r {
                year = Some(ayear);
            }
        }
        println!(
            "{}: {artist:?}; {album_title:?}; {year:?}",
            album.overview()
        );

        Ok((
            AlbumInfo {
                artist: artist.context("no artist")?.to_string(),
                title: album_title.context("no album_title")?.to_string(),
                year,
            },
            limit,
        ))
    } else {
        bail!(
            "Failed to find matching discogs result for {}",
            album.overview()
        );
    }
}
