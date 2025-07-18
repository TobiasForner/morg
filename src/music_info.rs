use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use distance::levenshtein;
use json::JsonValue;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{Album, music_tags::set_tags};

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Keys {
    pub key: String,
    pub secret: String,
}

impl Keys {
    fn keys_file() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("TF", "TF", "morg")
            .context("Failed to construct config path!")?;
        let keys_file = dirs.config_local_dir().join("keys.txt");
        Ok(keys_file)
    }
    pub fn parse() -> Result<Self> {
        let keys_file = Keys::keys_file()?;
        let text = std::fs::read_to_string(&keys_file)
            .context(format!("Could not read keys from {keys_file:?}"))?
            .replace("\r\n", "\n");
        toml::from_str(&text).context("Could not parse keys")
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
        ("page", "1".to_string()),
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
    let content = runtime.block_on(res.text());
    let parsed = json::parse(&content.unwrap()).unwrap();

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
            let mut file = std::fs::File::create(cover_path)?;
            reqwest::blocking::get(cover_url)?.copy_to(&mut file)?;
        }
        Ok(limit)
    } else {
        bail!("Failed to find matching discogs result for {album:?}");
    }
}

/// returns the number of requests that are allowed in the current window (atm this is a minute and
/// morg is allowed to use 60 requests per minute)
pub fn set_music_info(album: &Album) -> Result<i32> {
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
        println!("{artist:?}; {album_title:?}; {year:?}");
        set_tags(album, album_title, artist, year)?;
        Ok(limit)
    } else {
        bail!("Failed to find matching discogs result for {album:?}");
    }
}
