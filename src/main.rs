use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use fs_extra::dir::CopyOptions;
use indicatif::ProgressIterator;
use music_info::MusicInfoCache;
use music_tags::set_tags;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    fs::read_dir,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time,
};

mod album;
mod location;
mod music_info;
mod music_tags;
use crate::{
    album::{Album, path_to_details},
    location::{AdbLocation, DirLocation, Location},
    music_info::AlbumInfo,
    music_tags::parse_track_info,
};
use crate::{
    album::{albums_in_dir, create_source_album_lookup},
    music_tags::set_missing_tags,
};

use clap::{Parser, Subcommand, ValueEnum};

use crate::music_info::download_cover_file;

const IMAGE_EXTENSIONS: [&str; 3] = ["jpeg", "jpg", "png"];
const MUSIC_EXTENSIONS: [&str; 4] = ["mp3", "flac", "wav", "m4a"];

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// manipulates the config
    Config {
        #[command(subcommand)]
        subcommand: ConfigCommands,
    },
    /// check your configured directories for issues like duplicate albums, albums that are nested
    /// too deeply and many more
    Check,
    /// sync files in the sources to the destination directories. If a suitable ADB connection can
    /// be established, the files are also synced to the first ADB device
    Sync,
    /// Uses discogs to set music tags (metadata)
    CleanUpTags {
        dir: PathBuf,
        #[arg(short, long)]
        no_cache: bool,
    },
    /// Uses discogs to download cover files. The cover files will be stored in the album directory
    FillInCoverFiles {
        dir: PathBuf,
        #[arg(short, long)]
        overwrite: bool,
    },
    /// WIP: fixes some issues in the file setup
    Fix,
    /// Just for internal testing purposes
    Test,
    /// Lists the albums found in src that are missing in dst
    Diff { src: PathBuf, dst: PathBuf },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// add a directory to the sources list
    AddSource {
        #[arg()]
        directory: PathBuf,
    },
    /// Add an ADB source
    AddADB {
        ft: FileType,
        #[clap(default_value_t = false)]
        allow_any: bool,
    },
    /// add a directory to the destination list
    AddDest {
        #[arg()]
        directory: PathBuf,
        ft: FileType,
        #[clap(default_value_t = false)]
        allow_any: bool,
    },
    /// Prints the config file location
    PrintFile,
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum FileType {
    M4A,
    MP3,
    Wav,
    Flac,
}

impl FileType {
    fn is_lossless(&self) -> bool {
        use FileType::*;
        matches!(self, Wav | Flac)
    }
}

impl ValueEnum for FileType {
    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        use FileType::*;
        Some(
            match self {
                M4A => "m4a",
                MP3 => "mp3",
                Wav => "wav",
                Flac => "flac",
            }
            .into(),
        )
    }
    fn value_variants<'a>() -> &'a [Self] {
        use FileType::*;
        &[M4A, MP3, Wav, Flac]
    }
}

impl Display for FileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.to_possible_value() {
            Some(v) => f.write_str(v.get_name()),
            None => Err(std::fmt::Error {}),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct DirConfig {
    source_directories: Vec<PathBuf>,
    /// dest, ft, allow_any (fallback option if ft is not available)
    destinations: Vec<(Destination, FileType, bool)>,
}

impl DirConfig {
    fn read() -> Result<Self> {
        let cfg_file = DirConfig::config_file();
        let txt = std::fs::read_to_string(&cfg_file)
            .context(format!("Failed to read config from {cfg_file:?}"));
        if let Ok(txt) = txt {
            toml::from_str(&txt).context("Failed to parse config!")
        } else {
            Ok(DirConfig {
                source_directories: vec![],
                destinations: vec![],
            })
        }
    }

    fn write(&self) -> Result<()> {
        let txt = toml::to_string(self)?;
        std::fs::write(DirConfig::config_file(), txt)?;
        Ok(())
    }

    fn config_file() -> PathBuf {
        let pd = ProjectDirs::from("TF", "TF", "morg").expect("The project dir should be valid!");
        let cfg_dir = pd.config_dir();
        if !cfg_dir.exists() {
            let r = std::fs::create_dir_all(cfg_dir);
            if r.is_err() {
                println!("ERROR: Failed to create directory {cfg_dir:?}");
            }
        }
        pd.config_dir().join("config.toml")
    }
}

#[derive(Clone, Deserialize, Serialize)]
enum Destination {
    PathDest(PathBuf),
    ADBDest,
}

fn main() {
    let res = run();
    if res.is_err() {
        println!("ERROR: {res:?}");
    }
}

fn run() -> Result<()> {
    let args = Cli::parse();
    match args.command {
        Commands::Config { subcommand } => {
            use ConfigCommands::*;
            match subcommand {
                AddDest {
                    directory,
                    ft,
                    allow_any,
                } => {
                    let mut config = DirConfig::read()?;
                    config
                        .destinations
                        .push((Destination::PathDest(directory), ft, allow_any));
                    config.write()?;
                }
                AddSource { directory } => {
                    let mut config = DirConfig::read()?;
                    config.source_directories.push(directory);
                    config.write()?;
                }
                AddADB { ft, allow_any } => {
                    let mut config = DirConfig::read()?;
                    config
                        .destinations
                        .push((Destination::ADBDest, ft, allow_any));
                    config.write()?;
                }
                PrintFile => {
                    println!("{:?}", DirConfig::config_file())
                }
            }
            Ok(())
        }
        Commands::Test => {
            /*let mut album = Album::new(
                "Who Made Who".to_string(),
                "AC/DC".to_string(),
                vec![],
                PathBuf::new(),
                vec![],
                "Who Made Who".to_string(),
                "AC/DC".to_string(),
            );
            download_cover_file(&mut album)?;
            //set_music_info(&album)?;*/
            let res = path_to_details(
                PathBuf::from_str(
                    "G:\\Dopamine_Audio\\Poppy\\Poppy - Choke [FLAC]\\Poppy - Choke - 05 The Holy Mountain.flac",
                )?,
                PathBuf::from_str("G:\\Dopamine_Audio")?,
            );
            println!("{res:?}");
            Ok(())
        }
        Commands::Sync => {
            let config = DirConfig::read()?;
            let mut destinations = config.destinations.clone();
            // sync to sources first
            destinations.sort_by_key(|d| match &d.0 {
                Destination::PathDest(p) => {
                    if config.source_directories.contains(p) {
                        0
                    } else {
                        1
                    }
                }
                Destination::ADBDest => 1,
            });

            destinations
                .iter()
                .for_each(|(dest, ft, allow_any)| match dest {
                    Destination::PathDest(p) => {
                        println!("===== Syncing to dir {p:?} =====");
                        let mut loc = DirLocation::new(p.to_path_buf());
                        sync_to_loc(&mut loc, ft, &config, *allow_any);
                    }
                    Destination::ADBDest => {
                        println!("===== Syncing to ADB devce =====");
                        let loc = AdbLocation::new();
                        if let Ok(mut loc) = loc {
                            sync_to_loc(&mut loc, ft, &config, *allow_any);
                        } else {
                            println!("{loc:?}\nSkipping this location.");
                        }
                    }
                });
            Ok(())
        }
        Commands::Check => {
            let config = DirConfig::read()?;
            let dirs_to_handle: HashSet<PathBuf> = config
                .source_directories
                .iter()
                .chain(config.destinations.iter().filter_map(|d| {
                    if let (Destination::PathDest(p), _, _) = d {
                        Some(p)
                    } else {
                        None
                    }
                }))
                .cloned()
                .collect();
            let mut all_albums = Vec::new();
            let mut albums_by_root = HashMap::new();
            // check whether an album path is contained in another one
            dirs_to_handle.iter().for_each(|dir| {
                let albums = albums_in_dir(dir);
                albums_by_root.insert(dir.clone(), albums.clone());
                albums.iter().enumerate().for_each(|(i, a)| {
                    all_albums.push(a.clone());

                    let mut cache = MusicInfoCache::load(false).unwrap();

                    if let Ok(album_info) = cache.get_album_info(a) {
                        a.tracks.iter().for_each(|t| {
                            let track_info = parse_track_info(t, a, &album_info);
                            if let Some(tn) = track_info.track_number {
                                let tn = tn.to_string();
                                if track_info.title.starts_with(&tn)
                                    || track_info.title.starts_with(&format!("0{tn}"))
                                {
                                    println!(
                                        "Track {t} of album {} starts with its track number",
                                        a.overview()
                                    )
                                }
                            }
                        });
                    }
                    if let Some((_, a2)) = albums
                        .iter()
                        .enumerate()
                        .find(|(j, a2)| i != *j && a.dir_path.starts_with(&a2.dir_path))
                    {
                        println!(
                            "Album {} is in a subdir of album {}",
                            a.overview(),
                            a2.overview()
                        );
                    }
                    if a.tracks.is_empty() {
                        println!("Album {} does not contain any tracks!", a.overview());
                    } else if a.file_type().is_none() {
                        println!(
                            "Album {} contains tracks with multiple filetypes",
                            a.overview()
                        );
                    }
                });
            });

            // check for albums with the same contents, but different key
            all_albums
                .iter()
                .filter(|a| !a.tracks.is_empty())
                .enumerate()
                .for_each(|(i, a1)| {
                    all_albums[i + 1..]
                        .iter()
                        .filter(|a2| a1.key() != a2.key() && a1.tracks == a2.tracks)
                        .for_each(|a2| {
                            println!(
                                "Found duplicate albums: {} ({}) and {} ({})",
                                a1.overview(),
                                a1.key(),
                                a2.overview(),
                                a2.key()
                            )
                        });
                });

            // check for symlinks in source directories
            let mut pos = 0;
            let mut dirs_to_handle: Vec<PathBuf> = config.source_directories.clone();
            while pos < dirs_to_handle.len() {
                let dir = &dirs_to_handle[pos];
                if let Ok(items) = read_dir(dir) {
                    for child in items {
                        if let Ok(child) = child
                            && let Ok(ft) = child.file_type()
                        {
                            if ft.is_symlink() {
                                println!("{child:?} is a symlink")
                            } else if ft.is_dir() {
                                dirs_to_handle.push(child.path().to_path_buf());
                            }
                        }
                    }
                }
                pos += 1;
            }

            // check for albums whose directory is nested too deeply
            // this is the case for albums that are more than two directories deep inside their
            // root directory
            albums_by_root.iter().for_each(|(root, albums)| {
                albums.iter().for_each(|a| {
                    let rel = pathdiff::diff_paths(&a.dir_path, root);
                    if let Some(rel) = rel {
                        let comps: Vec<String> = rel
                            .components()
                            .map(|c| c.as_os_str().to_string_lossy().to_string())
                            .collect();
                        if comps.len() > 2 {
                            println!(
                                "The directory of album {} is nested too deeply.",
                                a.overview()
                            );
                        }
                    }
                })
            });

            Ok(())
        }
        Commands::CleanUpTags { dir, no_cache } => {
            println!("Loading albums...");
            let albums = albums_in_dir(&dir);
            println!("Loading cache...");
            let mut cache = MusicInfoCache::load(no_cache)?;
            println!("Setting tags...");
            albums.iter().progress().for_each(|a| {
                let info = cache.get_album_info(a);
                if let Ok(info) = info {
                    let success = set_tags(a, &info);
                    if success.is_err() {
                        println!("Failed to set album tags for {}: {success:?}", a.overview());
                    }
                } else {
                    println!("Failed to get album info: {info:?}; Falling back to album...");
                    let album_info = AlbumInfo {
                        artist: a.artist.clone(),
                        title: a.title.clone(),
                        year: None,
                    };
                    let success = set_missing_tags(a, &album_info);
                    if success.is_err() {
                        println!("Failed to set album tags for {}: {success:?}", a.overview());
                    }
                }
            });
            Ok(())
        }
        Commands::FillInCoverFiles { dir, overwrite } => {
            let mut albums = albums_in_dir(&dir);
            albums
                .iter_mut()
                .filter(|a| overwrite || a.cover_files.is_empty())
                .for_each(|a| {
                    let res = download_cover_file(a);
                    if let Ok(limit) = res {
                        println!("Downloaded cover file for {}", a.overview());
                        if limit <= 1 {
                            println!("Waiting 60s to avoid rate limit...");

                            std::thread::sleep(time::Duration::from_secs(60));
                        }
                    } else {
                        println!("Failed to download cover file: {res:?}");
                    }
                });
            Ok(())
        }
        Commands::Fix => {
            let config = DirConfig::read().unwrap();
            // check for symlinks in source directories
            let mut pos = 0;
            let mut dirs_to_handle: Vec<PathBuf> = config.source_directories.clone();
            while pos < dirs_to_handle.len() {
                let dir = &dirs_to_handle[pos];
                if let Ok(items) = read_dir(dir) {
                    for child in items {
                        if let Ok(child) = child
                            && let Ok(ft) = child.file_type()
                        {
                            if ft.is_symlink() {
                                println!("{child:?} is a symlink");
                                if let Ok(sl) = child.path().read_link() {
                                    println!("Deleting symlink {:?}", child.path());
                                    let _ = std::fs::remove_dir_all(child.path()).context("");
                                    let copy_options = CopyOptions::new();

                                    fs_extra::copy_items(&[&sl], child.path(), &copy_options)?;
                                }
                            } else if ft.is_dir() {
                                dirs_to_handle.push(child.path().to_path_buf());
                            }
                        }
                    }
                }
                pos += 1;
            }

            Ok(())
        }
        Commands::Diff { src, dst } => {
            let src_albums = albums_in_dir(&src);
            let dst_albums: HashMap<String, Album> = albums_in_dir(&dst)
                .into_iter()
                .map(|a| (a.key(), a))
                .collect();
            let mut missing_keys = HashSet::new();
            src_albums.iter().for_each(|a| {
                let key = a.key();
                if !dst_albums.contains_key(&key) && !missing_keys.contains(&key) {
                    println!("Album missing: {}", a.overview());
                    missing_keys.insert(key);
                }
            });
            Ok(())
        }
    }
}

/// tries to obtain a copy of album with file type `dest_ft`
fn get_ft_src_album(
    album: &Album,
    dest_ft: &FileType,
    album_lookup: &HashMap<(String, FileType), (Album, PathBuf)>,
) -> Option<Album> {
    if let Some((src_album, _src)) = album_lookup.get(&(album.key(), dest_ft.clone())) {
        return Some(src_album.clone());
    } else {
        // this is the order in which src_ft are tried for conversion
        let src_ft_order = [FileType::Flac, FileType::Wav, FileType::MP3, FileType::M4A];
        for ft in src_ft_order {
            if let Some((src_album, src)) = album_lookup.get(&(album.key(), ft.clone())) {
                println!(
                    "Found {ft:?} source album {:?}. Converting to {dest_ft:?}",
                    album.overview()
                );
                let res = convert_src_album(src, src_album, dest_ft);
                if let Ok(res) = res {
                    return Some(res);
                } else {
                    println!("Conversion {} -> {dest_ft} failed!", album.overview());
                }
            }
        }
    };
    None
}

/// simply copies the album files to the location in the desired file type
/// does NOT delete any files in the location
fn ensure_album_is_in_location(
    src_album: &Album,
    dest_ft: &FileType,
    album_lookup: &HashMap<(String, FileType), (Album, PathBuf)>,
    location: &mut dyn Location,
    allow_any: bool,
) -> Result<FileType> {
    println!(
        "Copying source album {} to location {}",
        src_album.overview(),
        location.to_string()
    );

    let new_src_album = get_ft_src_album(src_album, dest_ft, album_lookup);
    if let Some(src_album) = new_src_album {
        println!("Found source album {}", src_album.overview());
        location.copy_full_album(&src_album)?;
        Ok(dest_ft.clone())
    } else if let Some(ft) = src_album.file_type()
        && allow_any
    {
        location.copy_full_album(src_album)?;
        Ok(ft)
    } else {
        bail!(
            "Failed to find proper source fitting source album for {} [{:?}]. dest_ft is {dest_ft}, allow_any={allow_any}",
            src_album.overview(),
            src_album.file_type()
        )
    }
}

fn convert_src_album(src: &Path, src_album: &Album, dest_ft: &FileType) -> Result<Album> {
    let Some(src_ft) = src_album.file_type() else {
        bail!(
            "Failed to determine filetype of source album {}",
            src_album.overview()
        )
    };
    if dest_ft.is_lossless() && !src_ft.is_lossless() {
        bail!(
            "Converting a lossy music format to a lossless one ({dest_ft}) is prohibited! src_album: {}",
            src_album.overview()
        )
    }
    let desired_ft = dest_ft.to_possible_value().expect("");
    let desired_ft = desired_ft.get_name();

    let new_src_album_dir = src_album.album_dir_with_ft(src.to_path_buf(), &Some(dest_ft.clone()));

    let create_album_dir = || {
        if !new_src_album_dir.exists() {
            std::fs::create_dir_all(&new_src_album_dir)
                .context(format!("Failed to create {new_src_album_dir:?}"))?;
        }
        Ok::<(), anyhow::Error>(())
    };

    let copy_cover_files = || {
        src_album.cover_files.iter().for_each(|cf| {
            let cf_name = cf.file_name().expect("cover files muts have a file name!");
            let cf_dest = new_src_album_dir.join(cf_name);
            println!("COPY: {cf:?} -> {cf_dest:?}");
            let r = std::fs::copy(cf, &cf_dest);
            if r.is_err() {
                println!("Cover copy failed: {r:?}");
            }
        });
    };
    let get_input_args = |full_input_track_path: &PathBuf| {
        vec![
            "-i".to_string(),
            full_input_track_path.to_str().expect("").to_string(),
        ]
    };
    let get_output_args = |full_output_track_path: &PathBuf| match dest_ft {
        FileType::MP3 => {
            let tmp: Vec<String> = [
                "-ab",
                "320k",
                "-map_metadata",
                "0",
                "-id3v2_version",
                "3",
                "-write_id3v1",
                "1",
                full_output_track_path.to_str().expect(""),
            ]
            .iter()
            .map(|a| a.to_string())
            .collect();
            Ok(tmp)
        }
        FileType::Flac => Ok(vec![
            full_output_track_path
                .to_str()
                .context(format!(
                    "Failed to convert {full_output_track_path:?} to string"
                ))?
                .to_string(),
        ]),
        ft => bail!("NOT IMPLEMENTED: conversion to {ft:?}"),
    };
    let mut new_tracks = vec![];
    create_album_dir()?;
    copy_cover_files();
    let src_ft_str = src_ft
        .to_possible_value()
        .expect("src_ft should have a value attached");
    let src_ft_str = src_ft_str.get_name();
    src_album.tracks.iter().for_each(|t| {
        let full_path = src_album.dir_path.join(t);
        let t_new = t.replace(&format!(".{src_ft_str}"), &format!(".{desired_ft}"));
        let dst_path = new_src_album_dir.join(&t_new);
        println!("Track: {full_path:?} --> {dst_path:?}");
        let mut args = get_input_args(&full_path);
        if let Ok(mut output_args) = get_output_args(&dst_path) {
            args.append(&mut output_args);
        }
        Command::new("ffmpeg")
            .args(&args)
            .output()
            .expect("failed to convert {full_path:?}");
        let track = dst_path
            .file_name()
            .expect("Destination music file should have a file_name")
            .to_str()
            .expect("")
            .to_string();
        new_tracks.push(track);
    });
    if new_tracks.len() == src_album.tracks.len() {
        Ok(Album::new(
            src_album.title.clone(),
            src_album.artist.clone(),
            new_tracks,
            new_src_album_dir,
            src_album.cover_files.clone(),
            src_album.parsed_title.clone(),
            src_album.parsed_artist.clone(),
        ))
    } else {
        bail!("Failed to convert src album: {src_album:?} --> {new_src_album_dir:?} ");
    }
}

fn sync_to_loc(location: &mut dyn Location, ft: &FileType, config: &DirConfig, allow_any: bool) {
    println!("Loading source albums...");
    let album_lookup = create_source_album_lookup(&config.source_directories);
    println!("Loaded source albums.");
    let albums = location.albums().unwrap();
    let mut albums_in_loc = HashSet::new();
    let copy_full_album =
        |location: &mut dyn Location,
         album: &Album,
         albums_in_loc: &mut HashSet<(String, FileType)>| {
            let res = ensure_album_is_in_location(album, ft, &album_lookup, location, allow_any);
            if let Ok(ft) = res {
                albums_in_loc.insert((album.key(), ft.clone()));
            } else {
                println!("{res:?}");
            }
        };

    // try to replace albums with proper filetypes
    albums.iter().for_each(|a| {
        if let Some(aft) = a.file_type() {
            // create proper source album
            let src_album = get_ft_src_album(a, ft, &album_lookup);

            // copy files
            if let Some(src_album) = src_album {
                if aft != *ft {
                    if !albums
                        .iter()
                        .any(|a2| a2.key() == a.key() && a2.file_type() == Some(ft.clone()))
                    {
                        println!(
                            "Found {} with wrong filetype (is {aft:?}, but should be {ft:?})",
                            a.overview()
                        );
                        println!(
                            "Will attempt to delete album in destination {:?}",
                            a.dir_path
                        );
                        let _ = location.del_album(a);
                        copy_full_album(location, &src_album, &mut albums_in_loc);
                    }
                } else {
                    albums_in_loc.insert((a.key(), aft.clone()));
                    location.copy_missing_files(&src_album, a);
                }
            } else {
                println!("Did not find {ft:?} source album for {}", a.overview());
                albums_in_loc.insert((a.key(), aft.clone()));
            }
        } else {
            println!("ERROR: Failed to determine file type of {}", a.overview());
        }
    });
    // copy over missing albums
    let album_lookup = create_source_album_lookup(&config.source_directories);
    album_lookup.values().for_each(|(album, _)| {
        if !albums_in_loc.iter().any(|(ak, _)| *ak == album.key()) {
            copy_full_album(location, album, &mut albums_in_loc);
        }
    });
}
