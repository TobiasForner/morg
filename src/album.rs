use crate::FileType;
use crate::IMAGE_EXTENSIONS;
use crate::MUSIC_EXTENSIONS;
use crate::music_tags::get_track_tags;
use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use counter::Counter;
use indicatif::ProgressIterator;
use pathdiff::diff_paths;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::read_dir;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Album {
    pub title: String,
    pub artist: String,
    pub tracks: Vec<String>,
    pub dir_path: PathBuf,
    pub cover_files: Vec<PathBuf>,
    pub parsed_title: String,
    pub parsed_artist: String,
}

impl Album {
    pub fn new(
        title: String,
        artist: String,
        tracks: Vec<String>,
        dir_path: PathBuf,
        cover_files: Vec<PathBuf>,
        parsed_title: String,
        parsed_artist: String,
    ) -> Self {
        Album {
            title,
            artist,
            tracks,
            dir_path,
            cover_files,
            parsed_title,
            parsed_artist,
        }
    }

    pub fn overview(&self) -> String {
        format!(
            "{} - {} ({:?}; {} tracks, {} cover files)",
            self.artist,
            self.title_without_filetype(),
            self.dir_path,
            self.tracks.len(),
            self.cover_files.len()
        )
    }

    pub fn title_without_filetype(&self) -> String {
        if let Some(ft) = self.file_type()
            && let Some(ft) = ft.to_possible_value()
        {
            let ft = ft.get_name();
            return self
                .title
                .replace(&format!("[{ft}]"), "")
                .trim()
                .to_string();
        }
        self.title.clone()
    }

    pub fn album_dir_with_ft(&self, root_dir: PathBuf, ft: &Option<FileType>) -> PathBuf {
        let title = if let Some(ft) = ft {
            format!(
                "{} [{}]",
                self.parsed_title,
                ft.to_possible_value().unwrap().get_name()
            )
        } else {
            self.parsed_title.to_string()
        };
        root_dir.join(&self.parsed_artist).join(title)
    }

    pub fn key(&self) -> String {
        format!("{}###{}", self.parsed_artist, self.parsed_title)
    }

    pub fn file_type(&self) -> Option<FileType> {
        let mut file_types = HashSet::new();
        self.tracks.iter().for_each(|t| {
            if let Some((_, ft)) = t.rsplit_once('.') {
                file_types.insert(ft.to_string());
            }
        });
        if file_types.len() == 1 {
            let ft = file_types.iter().next();
            if let Some(ft) = ft {
                FileType::value_variants()
                    .iter()
                    .find(|vv| {
                        if let Some(s) = vv.to_possible_value() {
                            s.get_name() == ft
                        } else {
                            false
                        }
                    })
                    .cloned()
            } else {
                None
            }
        } else {
            None
        }
    }

    fn merge_with(&self, other: &Album) -> Result<Album> {
        if self.title == other.title
            && self.artist == other.artist
            && self.dir_path == other.dir_path
        {
            let mut tracks = HashSet::new();
            self.tracks.iter().for_each(|t| {
                tracks.insert(t.to_string());
            });
            other.tracks.iter().for_each(|t| {
                tracks.insert(t.to_string());
            });
            let mut tracks: Vec<String> = tracks.into_iter().collect();
            tracks.sort();
            let cover_files: HashSet<PathBuf> = self
                .cover_files
                .clone()
                .into_iter()
                .chain(other.cover_files.clone())
                .collect();
            let cover_files = cover_files.into_iter().collect();
            Ok(Album::new(
                self.title.clone(),
                self.artist.clone(),
                tracks,
                self.dir_path.clone(),
                cover_files,
                self.parsed_title.clone(),
                self.parsed_artist.clone(),
            ))
        } else {
            bail!("Failed to merge {self:?} and {other:?}!")
        }
    }

    fn finalize(&mut self) {
        let mut artists_counts: Counter<String> = Counter::new();
        self.tracks.iter().for_each(|t| {
            let track_path = self.dir_path.join(t);
            if let Ok(tags) = get_track_tags(&track_path)
                && let Some(artist) = tags.album_artist()
            {
                let artist = artist.to_string();
                artists_counts[&artist] += 1;
            }
        });
        self.parsed_title = self.title_without_filetype();

        let mc = artists_counts.most_common();
        if !mc.is_empty() {
            self.artist = mc[0].0.to_string();
        }
    }
}

pub fn create_source_album_lookup(
    source_dirs: &[PathBuf],
) -> HashMap<(String, FileType), (Album, PathBuf)> {
    let mut album_lookup = HashMap::new();
    source_dirs.iter().for_each(|sd| {
        let albums = albums_in_dir(sd);
        albums.into_iter().for_each(|a| {
            if let Some(ft) = a.file_type() {
                album_lookup.insert((a.key(), ft), (a.clone(), sd.clone()));
            }
        })
    });
    album_lookup
}

pub fn group_files_into_albums(file_paths: &[PathBuf], root: &Path) -> Vec<Album> {
    let mut album_lookup: HashMap<PathBuf, Album> = HashMap::new();
    file_paths.iter().progress().for_each(|mp| {
        if let Some(album_dir) = mp.parent() {
            let album_dir = album_dir.to_path_buf();
            let album = path_to_details(mp.into(), root.to_path_buf());
            if let Ok(album) = album {
                if let Some(a) = album_lookup.get(&album_dir) {
                    let merged = album.merge_with(a);
                    if let Ok(merged) = merged {
                        album_lookup.insert(album_dir, merged);
                    } else {
                        println!("ERROR: {merged:?}");
                    }
                } else {
                    album_lookup.insert(album_dir, album);
                };
            }
        }
    });
    println!("Finalizing albums...");
    album_lookup
        .into_values()
        .progress()
        .map(|mut a| {
            a.finalize();
            a
        })
        .collect()
}

pub fn path_to_details(path: PathBuf, root_dir: PathBuf) -> Result<Album> {
    let rel = diff_paths(&path, &root_dir).expect("path must be a child of root_dir!");
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => name.to_str().map(|s| s.to_string()),
            _ => None,
        })
        .collect();

    let (artist, album, file) = if parts.len() == 3 {
        (parts[0].clone(), parts[1].clone(), parts[2].clone())
    } else if parts.len() > 3 {
        let artist = parts[0].clone();
        let album = parts[1..parts.len() - 1].join(" - ");
        let track = parts[parts.len() - 1].clone();
        (artist, album, track)
    } else if parts.len() == 2 {
        if let Some((artist, album)) = parts[0].split_once(" - ") {
            (artist.to_string(), album.to_string(), parts[1].clone())
        } else {
            let artist = parts[0].clone();

            let rest = parts[1].replace(&format!("{artist} - "), "");
            if let Some((album, track)) = rest.rsplit_once(" - ") {
                (artist, album.to_string(), track.to_string())
            } else {
                bail!("Expected ' - ' delimiter between album name and track, but got {parts:?}");
            }
        }
    } else {
        bail!("Could not parse details from {path:?}!");
    };

    let mut album = album.trim().to_string();
    if let Some(ext) = MUSIC_EXTENSIONS
        .iter()
        .flat_map(|ext| {
            [ext.to_string(), ext.to_uppercase()].into_iter().chain(
                FileType::value_variants()
                    .iter()
                    .map(|ft| ft.to_possible_value().unwrap().get_name().to_string()),
            )
        })
        .find(|ext| album.ends_with(&format!("[{ext}]")))
    {
        album = album.replace(&format!("[{ext}]"), "").trim().to_string();
    }

    let cover_files = if is_image(&path) {
        vec![path.clone()]
    } else {
        vec![]
    };
    let mut tracks = vec![];
    if is_music(&path) {
        tracks.push(file);
    }
    let dir_path = path
        .parent()
        .context("file should have a parent!")?
        .to_path_buf();
    Ok(Album::new(
        album.to_string(),
        artist.clone(),
        tracks,
        dir_path,
        cover_files,
        album.to_string(),
        artist,
    ))
}

fn is_image(file: &Path) -> bool {
    let Some(ext) = file.extension() else {
        return false;
    };
    IMAGE_EXTENSIONS.iter().any(|e| ext == *e)
}

fn is_music(file: &Path) -> bool {
    let Some(ext) = file.extension() else {
        return false;
    };
    MUSIC_EXTENSIONS.iter().any(|e| ext == *e)
}

fn files_in_dir(root: &Path) -> Vec<PathBuf> {
    let mut res = vec![];
    read_dir(root)
        .unwrap_or_else(|_| panic!("root directory {root:?} does not exist!"))
        .for_each(|de| {
            let de = de.unwrap();
            if let Ok(ft) = de.file_type() {
                if ft.is_file() {
                    res.push(de.path().to_path_buf());
                } else if ft.is_dir() {
                    let mut rec = files_in_dir(&de.path());

                    res.append(&mut rec);
                }
            }
        });
    res
}

pub fn albums_in_dir(root: &Path) -> Vec<Album> {
    let files = files_in_dir(root);
    println!("Got albums in directory {root:?}");
    println!("Grouping files into albums...");
    group_files_into_albums(&files, root)
}
