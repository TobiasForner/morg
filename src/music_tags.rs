use std::path::PathBuf;

use anyhow::{Context, Result};
use audiotags::Tag;

use crate::Album;

pub fn set_tags(
    album: &Album,
    title: Option<&str>,
    artist: Option<&str>,
    year: Option<i32>,
) -> Result<()> {
    album.tracks.iter().try_for_each(|t| {
        let track_path = album.dir_path.join(t);
        let mut tag = Tag::new()
            .read_from_path(&track_path)
            .context(format!("Failed to read tags from {track_path:?}"))?;
        if let Some(title) = title {
            tag.set_album_title(title);
        }
        if let Some(artist) = artist {
            tag.set_album_artist(artist);
        }
        if let Some(year) = year {
            tag.set_year(year);
        }
        tag.write_to_path(
            track_path
                .to_str()
                .context("track path should be a valid string")?,
        )?;

        Ok(())
    })
}

pub fn get_track_tags(
    abs_track_path: &PathBuf,
) -> Result<Box<dyn audiotags::AudioTag + 'static + Send + Sync>> {
    let res = Tag::new()
        .read_from_path(abs_track_path)
        .context(format!("Failed to read tags from {abs_track_path:?}"));
    res
}
