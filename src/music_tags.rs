use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use audiotags::{AudioTag, FlacTag, Id3v2Tag, Tag};

use crate::{Album, FileType, music_info::AlbumInfo};

fn get_empty_mp3_tag() -> Box<dyn AudioTag + Send + Sync> {
    Box::new(Id3v2Tag::new())
}

fn get_empty_flac_tag() -> Box<dyn AudioTag + Send + Sync> {
    Box::new(FlacTag::new())
}

pub fn set_tags(album: &Album, album_info: &AlbumInfo) -> Result<()> {
    album.tracks.iter().try_for_each(|t| {
        let track_path = album.dir_path.join(t);
        let mut tag = match Tag::new().read_from_path(&track_path) {
            Ok(tag) => tag,
            Err(_) => match album.file_type() {
                Some(FileType::MP3) => get_empty_mp3_tag(),
                Some(FileType::Flac) => get_empty_flac_tag(),
                _ => bail!(""),
            },
        };

        //.context(format!("Failed to read tags from {track_path:?}"))?;
        tag.set_album_title(&album_info.title);
        tag.set_album_artist(&album_info.artist);
        if let Some(year) = album_info.year {
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
    Tag::new()
        .read_from_path(abs_track_path)
        .context(format!("Failed to read tags from {abs_track_path:?}"))
}
