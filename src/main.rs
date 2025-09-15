use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use indicatif::ProgressIterator;
use music_info::MusicInfoCache;
use music_tags::set_tags;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    io::BufWriter,
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
};
use crate::{
    album::{albums_in_dir, create_source_album_lookup},
    music_tags::set_missing_tags,
};

use adb_client::{ADBDeviceExt, ADBServerDevice};
use clap::{Parser, Subcommand, ValueEnum};

use crate::music_info::download_cover_file;

const IMAGE_EXTENSIONS: [&str; 3] = ["jpeg", "jpg", "png"];
const MUSIC_EXTENSIONS: [&str; 3] = ["mp3", "flac", "wav"];

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
    /// Just for internal testing purposes
    Test,
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
    MP3,
    Wav,
    Flac,
}

impl ValueEnum for FileType {
    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        use FileType::*;
        Some(
            match self {
                MP3 => "mp3",
                Wav => "wav",
                Flac => "flac",
            }
            .into(),
        )
    }
    fn value_variants<'a>() -> &'a [Self] {
        use FileType::*;
        &[MP3, Wav, Flac]
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
            // check whether an album path is contained in another one
            dirs_to_handle.iter().for_each(|dir| {
                let albums = albums_in_dir(dir);
                albums.iter().enumerate().for_each(|(i, a)| {
                    if let Some((_, a2)) = albums
                        .iter()
                        .enumerate()
                        .find(|(j, a2)| i != *j && a.dir_path.starts_with(&a2.dir_path))
                    {
                        println!(
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
    }
}

fn get_ft_src_album(
    album: &Album,
    dest_ft: &FileType,
    album_lookup: &HashMap<(String, FileType), (Album, PathBuf)>,
) -> Option<Album> {
    if let Some((src_album, _src)) = album_lookup.get(&(album.key(), dest_ft.clone())) {
        Some(src_album.clone())
    } else if let Some((src_album, src)) = album_lookup.get(&(album.key(), FileType::Flac))
        && *dest_ft != FileType::Flac
    {
        println!(
            "Found Flac source album {:?}. Converting to {dest_ft:?}",
            album.overview()
        );
        convert_src_album(src, src_album, dest_ft).ok()
    } else if let Some((_src_album, _src)) = album_lookup.get(&(album.key(), FileType::Wav))
        && *dest_ft != FileType::Wav
    {
        println!("Found wav source album {:?}", album.overview());
        println!("NOT IMPLEMENTED: Album conversion wav => mp3");
        None
    } else {
        None
    }
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
        "Copying missing files of source album {} to device",
        src_album.overview()
    );

    let new_src_album = get_ft_src_album(src_album, dest_ft, album_lookup);
    if let Some(src_album) = new_src_album {
        println!("Found source album {}", src_album.overview());
        let _ = location.copy_full_album(&src_album);
        Ok(dest_ft.clone())
    } else if let Some(ft) = src_album.file_type()
        && allow_any
    {
        let _ = location.copy_full_album(src_album);
        Ok(ft)
    } else {
        bail!("")
    }
}

fn convert_src_album(src: &Path, src_album: &Album, dest_ft: &FileType) -> Result<Album> {
    let desired_ft = dest_ft.to_possible_value().expect("");
    let desired_ft = desired_ft.get_name();

    let new_src_album_dir = src_album.album_dir_with_ft(src.to_path_buf(), &Some(dest_ft.clone()));

    let create_album_dir = || {
        if !new_src_album_dir.exists() {
            let _ = std::fs::create_dir_all(&new_src_album_dir);
        }
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
    let mut new_tracks = vec![];
    match src_album.file_type() {
        Some(FileType::Flac) => {
            match dest_ft {
                FileType::MP3 => {
                    copy_cover_files();
                    create_album_dir();
                    src_album.tracks.iter().for_each(|t| {
                        let full_path = src_album.dir_path.join(t);
                        let t_new = t.replace(".flac", &format!(".{desired_ft}"));
                        let dst_path = new_src_album_dir.join(&t_new);
                        println!("Track: {full_path:?} --> {dst_path:?}");
                        // TODO: invesigate whether/which other commands are required for different combinations of src and
                        // dest filetypes
                        Command::new("ffmpeg")
                            .args([
                                "-i",
                                full_path.to_str().expect(""),
                                "-ab",
                                "320k",
                                "-map_metadata",
                                "0",
                                "-id3v2_version",
                                "3",
                                "-write_id3v1",
                                "1",
                                dst_path.to_str().expect(""),
                            ])
                            .output()
                            .expect("failed to convert {full_path:?}");
                        let track = dst_path
                            .file_name()
                            .expect("Destination music file should have a file_name")
                            .to_str()
                            .expect("")
                            .to_string();
                        new_tracks.push(track);
                    })
                }
                _ => {
                    println!(
                        "TODO: implement conversion {:?} --> {dest_ft:?}",
                        src_album.file_type()
                    );
                }
            }
        }
        _ => {
            println!(
                "TODO: implement conversion {:?} --> {dest_ft:?}",
                src_album.file_type()
            );
        }
    }
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
        bail!("Failed to convert src album: {src_album:?} -->{new_src_album_dir:?} ");
    }
}

fn normalize_artist(artist: &str) -> String {
    artist.replace("/", " ")
}

fn del_album_on_device(adb_album: &Album, device: &mut ADBServerDevice) {
    let mut buf = BufWriter::new(Vec::new());
    let album_path = adb_album
        .dir_path
        .to_str()
        .expect("adb album path must be convertible to str");
    let album_path = format!("\"{album_path}\"");
    println!("Attempting to delete {album_path}");
    let command = vec!["rm", "-rf", &album_path];
    let _ = device.shell_command(&command, &mut buf);
    let bytes = buf.into_inner().unwrap();
    let out = String::from_utf8_lossy(&bytes).to_string();
    println!("{out}");
}

fn dir_exists_on_adb_device(device: &mut ADBServerDevice, path: &str) -> bool {
    let cmd = format!("if [ -d {path} ]; then echo 'Exists'; else echo 'Not found'; fi");
    let command: Vec<&str> = vec![&cmd];
    let mut buf = BufWriter::new(Vec::new());
    let _ = device.shell_command(&command, &mut buf);
    let bytes = buf.into_inner().unwrap();
    let out = String::from_utf8_lossy(&bytes).to_string();
    out.contains("exists")
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
                if aft != *ft
                    && !albums
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
