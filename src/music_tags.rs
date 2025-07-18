use anyhow::{Context, Result};
use audiotags::Tag;

use crate::Album;
pub fn set_artist_tag(album: &Album) -> Result<()> {
    album.tracks.iter().try_for_each(|t| {
        let track_path = album.dir_path.join(t);
        let mut tag = Tag::new()
            .read_from_path(&track_path)
            .context(format!("Failed to read tags from {track_path:?}"))?;
        tag.set_album_artist(&album.artist);
        tag.write_to_path(
            track_path
                .to_str()
                .context("track path should be a valid string")?,
        )?;

        Ok(())
    })
}
