use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use audiotags::{AudioTag, FlacTag, Id3v2Tag, Tag};
use regex::Regex;

use crate::{Album, FileType, music_info::AlbumInfo};

fn get_empty_mp3_tag() -> Box<dyn AudioTag + Send + Sync> {
    Box::new(Id3v2Tag::new())
}

fn get_empty_flac_tag() -> Box<dyn AudioTag + Send + Sync> {
    Box::new(FlacTag::new())
}

pub fn set_missing_tags(album: &Album, album_info: &AlbumInfo) -> Result<()> {
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

        if tag.album_title().is_none() {
            tag.set_album_title(&album_info.title);
        }
        if tag.album_artists().is_none() {
            tag.set_artist(&album_info.artist)
        }
        if let Some(year) = album_info.year {
            tag.set_year(year);
        }
        let number_re = Regex::new(r"(\d+-)?(\d+)").unwrap();
        if let Some(parts) = t.split_once(' ') {
            if let Some(capture) = number_re.captures(parts.0) {
                if let Some(c) = capture.get(1)
                    && let Ok(disc_num) = c.as_str().parse()
                    && tag.disc_number().is_none()
                {
                    tag.set_disc_number(disc_num);
                }
                if let Some(c) = capture.get(2)
                    && let Ok(track_num) = c.as_str().parse()
                    && tag.track_number().is_none()
                {
                    tag.set_track_number(track_num);
                }
            }
            if let Some((name, _)) = parts.1.rsplit_once('.')
                && tag.title().is_none()
            {
                let title = name.trim_start_matches("- ");
                let title = title
                    .replace(&format!("{} - ", album_info.artist), "")
                    .replace(&format!("{} - ", album.artist), "")
                    .replace(&format!("{} - ", album_info.title), "");
                let title = title.trim();
                tag.set_title(title);
            }
        }
        tag.write_to_path(
            track_path
                .to_str()
                .context("track path should be a valid string")?,
        )?;

        Ok(())
    })
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

        tag.set_album_title(&album_info.title);
        if tag.album_artists().is_none() {
            tag.set_artist(&album_info.artist)
        }
        if let Some(year) = album_info.year {
            tag.set_year(year);
        }
        let number_re = Regex::new(r"(\d+-)?(\d+)").unwrap();
        if let Some(parts) = t.split_once(' ') {
            if let Some(capture) = number_re.captures(parts.0) {
                if let Some(c) = capture.get(1)
                    && let Ok(disc_num) = c.as_str().parse()
                    && tag.disc_number().is_none()
                {
                    tag.set_disc_number(disc_num);
                }
                if let Some(c) = capture.get(2)
                    && let Ok(track_num) = c.as_str().parse()
                    && tag.track_number().is_none()
                {
                    tag.set_track_number(track_num);
                }
            }
            if let Some((name, _)) = parts.1.rsplit_once('.') {
                let title = name.trim_start_matches("- ");
                let title = title
                    .replace(&format!("{} - ", album_info.artist), "")
                    .replace(&format!("{} - ", album.artist), "")
                    .replace(&format!("{} - ", album_info.title), "");
                let title = title.trim();
                tag.set_title(title);
            }
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
