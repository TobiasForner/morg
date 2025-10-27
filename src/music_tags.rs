use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use audiotags::{AudioTag, FlacTag, Id3v2Tag, Tag};
use regex::Regex;

use crate::{Album, FileType, music_info::AlbumInfo};

pub fn set_missing_tags(album: &Album, album_info: &AlbumInfo) -> Result<()> {
    album.tracks.iter().try_for_each(|t| {
        let track_path = album.dir_path.join(t);
        let mut tag = get_tag(&track_path, album)?;

        if tag.album_title().is_none() {
            tag.set_album_title(&album_info.title);
        }
        if let Some(aa) = tag.album_artist()
            && aa.is_empty()
        {
            tag.set_album_artist(&album_info.artist);
        } else if tag.album_artist().is_none() {
            tag.set_album_artist(&album_info.artist);
        }
        if tag.artist().is_none() {
            tag.set_artist(&album_info.artist)
        }
        let track_info = parse_track_info(t, album, album_info);
        if tag.title().is_none() {
            tag.set_title(&track_info.title);
        }
        if let Some(dn) = track_info.disc_number
            && tag.disc_number().is_none()
        {
            tag.set_disc_number(dn);
        }
        if let Some(tn) = track_info.track_number
            && tag.track_number().is_none()
        {
            tag.set_track_number(tn);
        }
        tag.write_to_path(
            track_path
                .to_str()
                .context("track path should be a valid string")?,
        )?;

        Ok(())
    })
}

pub struct TrackInfo {
    pub title: String,
    pub disc_number: Option<u16>,
    pub track_number: Option<u16>,
}

pub fn parse_track_info(rel_track_path: &str, album: &Album, album_info: &AlbumInfo) -> TrackInfo {
    let mut res = TrackInfo {
        title: "".to_string(),
        disc_number: None,
        track_number: None,
    };
    let number_re = Regex::new(r"(\d+-)?(\d+)").unwrap();
    if let Some(parts) = rel_track_path.split_once(' ') {
        if let Some(capture) = number_re.captures(parts.0) {
            if let Some(c) = capture.get(1)
                && let Ok(disc_num) = c.as_str().parse()
            {
                res.disc_number = Some(disc_num);
            }
            if let Some(c) = capture.get(2)
                && let Ok(track_num) = c.as_str().parse()
            {
                res.track_number = Some(track_num);
            }
        }
        if let Some((name, _)) = parts.1.rsplit_once('.') {
            let title = name.trim_start_matches("- ");
            let title = title
                .replace(&format!("{} - ", album_info.artist), "")
                .replace(&format!("{} - ", album.artist), "")
                .replace(&format!("{} - ", album_info.title), "")
                .replace(&format!("{} - ", album.parsed_artist), "")
                .replace(&format!("{} - ", album.parsed_title), "");
            let title = title.trim();
            res.title = title.to_string();
        }
    }
    res
}

fn get_tag(track_path: &PathBuf, album: &Album) -> Result<Box<dyn AudioTag + Send + Sync>> {
    let tag = match Tag::new().read_from_path(track_path) {
        Ok(tag) => tag,
        Err(_) => {
            let tag: Box<dyn AudioTag + Send + Sync> = match album.file_type() {
                Some(FileType::MP3) => Box::new(Id3v2Tag::new()),
                Some(FileType::Flac) => Box::new(FlacTag::new()),
                Some(ft) => bail!("Could not create tag object for file type {ft}."),
                None => bail!("Failed to create tag: file type of album {album:?} is not known."),
            };
            tag
        }
    };
    Ok(tag)
}

pub fn set_tags(album: &Album, album_info: &AlbumInfo) -> Result<()> {
    let mut first = true;
    album.tracks.iter().try_for_each(|t| {
        let track_path = album.dir_path.join(t);
        let mut tag = get_tag(&track_path, album)?;

        tag.set_album_title(&album_info.title);
        if first {
            println!("aa: {:?}", tag.album_artist());
        }
        first = false;
        if tag.album_artist().is_none() {
            tag.set_album_artist(&album_info.artist);
        } else if let Some(aa) = tag.album_artist()
            && aa.is_empty()
        {
            tag.set_album_artist(&album_info.artist);
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

#[test]
fn test_parse_track_info() {
    use crate::album::path_to_details;
    use std::str::FromStr;
    let album = path_to_details(
        PathBuf::from_str("G:\\Music\\Poppy\\Poppy - Negative Spaces [MP3]\\Poppy - Negative Spaces - 04 yesterday.mp3").unwrap(),
        PathBuf::from_str("G:\\Music").unwrap(),
    )
    .unwrap();
    assert_eq!(album.parsed_artist, "Poppy".to_string());
    assert_eq!(album.parsed_title, "Negative Spaces".to_string());
}
