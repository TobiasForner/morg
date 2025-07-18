use anyhow::{Context, Result, anyhow, bail};
use directories::ProjectDirs;
use fs_extra::dir::CopyOptions;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time,
};

mod album;
mod music_info;
mod music_tags;
use crate::album::Album;
use crate::album::{albums_in_dir, create_source_album_lookup, group_files_into_albums};

use adb_client::{ADBDeviceExt, ADBServer, ADBServerDevice};
use clap::{Parser, Subcommand, ValueEnum};

use crate::music_info::{download_cover_file, set_music_info};

const IMAGE_EXTENSIONS: [&str; 2] = ["jpg", "png"];
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
    CleanUpTags {
        dir: PathBuf,
    },
    FillInCoverFiles {
        dir: PathBuf,
    },
    Test,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// add a directory to the sources list
    AddSource {
        #[arg()]
        directory: PathBuf,
    },
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

#[derive(Deserialize, Serialize)]
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
            let mut album = Album::new(
                "Who Made Who".to_string(),
                "AC/DC".to_string(),
                vec![],
                PathBuf::new(),
                vec![],
            );
            download_cover_file(&mut album)?;
            //set_music_info(&album)?;
            Ok(())
        }
        Commands::Sync => {
            let config = DirConfig::read()?;
            // gather available albums and filetypes in all sources, grouped by (title, ft)
            let mut album_lookup = HashMap::new();
            config.source_directories.iter().for_each(|sd| {
                let albums = albums_in_dir(sd);
                albums.into_iter().for_each(|a| {
                    if let Some(ft) = a.file_type() {
                        album_lookup.insert((a.title.clone(), ft), a.clone());
                    }
                })
            });
            // iterate over desinations, for each dest
            // 1. check the present albums
            // 2. if an album is present, but in the wrong ft
            // 2.1 check if any source has that album in either the correct ft or in a lossless one
            //   (if only the latter: create a copy with the desired ft in the source dir, and add
            //   it to the lookup)
            //   if yes: delete the album in the destination
            // 3. gather a list of the albums that are present in the destination
            // 4. determine missing ones and copy over from the first suitable source
            config
                .destinations
                .iter()
                .for_each(|(dest, ft, allow_any)| match dest {
                    Destination::PathDest(p) => {
                        sync_to_dir(p, ft, &config, *allow_any);
                    }
                    Destination::ADBDest => {
                        sync_to_device(ft, &config, *allow_any);
                    }
                });
            Ok(())
        }
        Commands::CleanUpTags { dir } => {
            let albums = albums_in_dir(&dir);
            albums.iter().for_each(|a| {
                let res = set_music_info(a);
                if let Ok(limit) = res {
                    if limit <= 1 {
                        println!("Waiting 60s to avoid rate limit...");

                        std::thread::sleep(time::Duration::from_secs(60));
                    }
                } else {
                    println!("Failed to set tags: {res:?}");
                }
            });
            Ok(())
        }
        Commands::FillInCoverFiles { dir } => {
            let mut albums = albums_in_dir(&dir);
            albums.iter_mut().for_each(|a| {
                if a.cover_files.is_empty() {
                    let res = download_cover_file(a);
                    if let Ok(limit) = res {
                        if limit <= 1 {
                            println!("Waiting 60s to avoid rate limit...");

                            std::thread::sleep(time::Duration::from_secs(60));
                        }
                    } else {
                        println!("Failed to download cover file: {res:?}");
                    }
                }
            });
            Ok(())
        }
    }
}

fn sync_to_dir(dest_dir: &Path, ft: &FileType, config: &DirConfig, allow_any: bool) {
    let album_lookup = create_source_album_lookup(&config.source_directories);
    let albums = albums_in_dir(dest_dir);
    let mut albums_in_dir = HashSet::new();

    // try to replace albums with proper filetypes
    albums.iter().for_each(|a| {
        if let Some(aft) = a.file_type()
            && aft != *ft
        {
            println!("Found {a:?} with wrong filetype (is {aft:?}, but should be {ft:?})");
            if let Some((src_album, _src)) =
                album_lookup.get(&(a.title.clone(), a.artist.clone(), ft.clone()))
            {
                println!("Found source album {src_album:?}");
                println!(
                    "Will attempt to delete album in destination {:?}",
                    a.dir_path
                );
                let _ = std::fs::remove_dir_all(&a.dir_path);
                println!("copying {:?} to {:?}!", src_album.dir_path, a.dir_path);
                let copy_options = CopyOptions::new();
                let _ = fs_extra::copy_items(&[&src_album.dir_path], &a.dir_path, &copy_options);
                albums_in_dir.insert((a.title.clone(), a.artist.clone(), ft.clone()));
            } else if let Some((src_album, src)) =
                album_lookup.get(&(a.title.clone(), a.artist.clone(), FileType::Flac))
            {
                println!("Found Flac source album {src_album:?} in destination {dest_dir:?}");
                if let Ok(src_album) = convert_src_album(src, src_album, ft) {
                    println!("Deleting {:?}!", a.dir_path);
                    let _ = std::fs::remove_dir_all(&a.dir_path);
                    println!("copying {:?} to {:?}!", src_album.dir_path, a.dir_path);
                    albums_in_dir.insert((a.title.clone(), a.artist.clone(), ft.clone()));
                }
            } else if let Some((src_album, _src)) =
                album_lookup.get(&(a.title.clone(), a.artist.clone(), FileType::Wav))
            {
                println!("Found wav source album {src_album:?}");
                println!("NOT IMPLEMENTED: Album conversion wav => mp3");
            }
        } else {
            albums_in_dir.insert((a.title.clone(), a.artist.clone(), ft.clone()));
        }
    });
    // copy over missing albums
    let album_lookup = create_source_album_lookup(&config.source_directories);
    album_lookup
        .iter()
        .for_each(|((album_title, album_artist, _), (album, _))| {
            if !albums_in_dir
                .iter()
                .any(|(at, aa, _)| at == album_title && *aa == *album.artist)
            {
                let res = ensure_album_is_in_dir(album, ft, &album_lookup, dest_dir, allow_any);
                if let Ok(ft) = res {
                    albums_in_dir.insert((
                        album_title.to_string(),
                        album_artist.to_string(),
                        ft.clone(),
                    ));
                } else {
                    println!("{res:?}");
                }
            }
        });
}

fn sync_to_device(ft: &FileType, config: &DirConfig, allow_any: bool) {
    let mut server = ADBServer::default();
    let devices = server.devices();

    println!("devices: {devices:?}");
    let Ok(mut device) = server.get_device() else {
        println!("ERROR: failed to get ADB device. Skipping this destination!");
        return;
    };

    let mut buf = BufWriter::new(Vec::new());
    let command = vec!["find", "/storage/emulated/0/Music", "-type", "f"];
    let _ = device.shell_command(&command, &mut buf);
    let bytes = buf.into_inner().unwrap();
    let out = String::from_utf8_lossy(&bytes).to_string();
    let music_paths: Vec<PathBuf> = out
        .lines()
        .map(|l| PathBuf::from_str(l).expect("each line should be a valid path!"))
        .collect();
    let pb = PathBuf::from_str("/storage/emulated/0/Music").unwrap();
    let albums = group_files_into_albums(&music_paths, pb.as_path());
    let mut albums_on_device = HashSet::new();

    let album_lookup = create_source_album_lookup(&config.source_directories);

    albums.iter().for_each(|a| {
        if let Some(aft) = a.file_type() {
            if aft != *ft {
                println!("Found {a:?} with wrong filetype (is {aft:?}, but should be {ft:?})");
                if let Some((src_album, _src)) =
                    album_lookup.get(&(a.title.clone(), a.artist.clone(), ft.clone()))
                {
                    println!("Found source album {src_album:?}");
                    println!(
                        "Will attempt to delete album on adb device at {:?}",
                        a.dir_path
                    );
                    println!("Deleting {:?} on ADB device!", a.dir_path);
                    del_album_on_device(a, &mut device);
                    println!(
                        "copying {:?} to {:?} on ADB device!",
                        src_album.dir_path, a.dir_path
                    );
                    adb_copy_album(src_album, &mut device);
                    albums_on_device.insert((a.title.clone(), a.artist.clone(), ft.clone()));
                } else if let Some((src_album, src)) =
                    album_lookup.get(&(a.title.clone(), a.artist.clone(), FileType::Flac))
                {
                    println!("Found Flac source album {src_album:?}");
                    if let Ok(src_album) = convert_src_album(src, src_album, ft) {
                        println!("Deleting {:?} on ADB device!", a.dir_path);
                        del_album_on_device(a, &mut device);
                        println!(
                            "copying {:?} to {:?} on ADB device!",
                            src_album.dir_path, a.dir_path
                        );
                        adb_copy_album(&src_album, &mut device);
                        albums_on_device.insert((a.title.clone(), a.artist.clone(), ft.clone()));
                    }
                } else if let Some((src_album, _src)) =
                    album_lookup.get(&(a.title.clone(), a.artist.clone(), FileType::Wav))
                {
                    println!("Found wav source album {src_album:?}");
                    println!("NOT IMPLEMENTED: Album conversion wav => mp3");
                }
            } else {
                albums_on_device.insert((a.title.clone(), a.artist.clone(), ft.clone()));
            }
        }
        if a.file_type().is_none() {
            println!("Failed to determine file type for {a:?}");
        }
    });

    let album_lookup = create_source_album_lookup(&config.source_directories);
    album_lookup
        .iter()
        .for_each(|((album_title, album_artist, _), (album, _))| {
            if !albums_on_device
                .iter()
                .any(|(at, aa, _)| at == album_title && *aa == *album.artist)
            {
                ensure_album_is_on_device(album, ft, &album_lookup, &mut device, allow_any);
                albums_on_device.insert((
                    album_title.to_string(),
                    album_artist.to_string(),
                    ft.clone(),
                ));
            }
        });
}

/// simply copies the album files to the device in the desired file type
/// does NOT delete any files on the device
fn ensure_album_is_on_device(
    src_album: &Album,
    dest_ft: &FileType,
    album_lookup: &HashMap<(String, String, FileType), (Album, PathBuf)>,
    device: &mut ADBServerDevice,
    allow_any: bool,
) -> Option<FileType> {
    if let Some((src_album, _src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        dest_ft.clone(),
    )) {
        println!("Found source album {src_album:?}");
        adb_copy_album(src_album, device);
        return Some(dest_ft.clone());
    } else if let Some((src_album, src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        FileType::Flac,
    )) {
        println!("Found Flac source album {src_album:?}");
        if convert_src_album(src, src_album, dest_ft).is_ok() {
            adb_copy_album(src_album, device);
            return Some(dest_ft.clone());
        }
    } else if let Some((src_album, _src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        FileType::Wav,
    )) {
        println!("Found wav source album {src_album:?}");
        println!("NOT IMPLEMENTED: Album conversion wav => {dest_ft:?}");
        if allow_any {
            adb_copy_album(src_album, device);
            return Some(FileType::Wav.clone());
        }
    } else if let Some((src_album, _src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        FileType::MP3,
    )) {
        println!("Found mp3 source album {src_album:?}");
        println!("NOT IMPLEMENTED: Album conversion mp3 => {dest_ft:?}");
        if allow_any {
            adb_copy_album(src_album, device);
            return Some(FileType::MP3.clone());
        }
    }

    None
}

/// simply copies the album files (files in the album's directory) to the directory in the desired file type
/// does NOT delete any files
fn ensure_album_is_in_dir(
    src_album: &Album,
    dest_ft: &FileType,
    album_lookup: &HashMap<(String, String, FileType), (Album, PathBuf)>,
    dest_dir: &Path,
    allow_any: bool,
) -> Result<FileType> {
    let dest_artist_dir = dest_dir.join(&src_album.artist);
    let copy_options = CopyOptions::new();
    if let Some((src_album, _src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        dest_ft.clone(),
    )) {
        println!("Found source album {src_album:?}");
        if !dest_artist_dir.exists() {
            let _ = std::fs::create_dir_all(&dest_artist_dir);
        }
        let _ = fs_extra::copy_items(&[&src_album.dir_path], dest_artist_dir, &copy_options);
        return Ok(dest_ft.clone());
    } else if let Some((src_album, src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        FileType::Flac,
    )) {
        println!("Found Flac source album {src_album:?}");
        if let Ok(src_album) = convert_src_album(src, src_album, dest_ft) {
            if !dest_artist_dir.exists() {
                let _ = std::fs::create_dir_all(&dest_artist_dir);
            }
            fs_extra::copy_items(&[&src_album.dir_path], dest_artist_dir, &copy_options)?;
            return Ok(dest_ft.clone());
        }
    } else if let Some((src_album, _src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        FileType::Wav,
    )) {
        println!("Found wav source album {src_album:?}");
        println!("NOT IMPLEMENTED: Album conversion wav => {dest_ft:?}");
        if allow_any {
            if !dest_artist_dir.exists() {
                let _ = std::fs::create_dir_all(&dest_artist_dir);
            }
            fs_extra::copy_items(&[&src_album.dir_path], &dest_artist_dir, &copy_options)?;
            println!("Copied over wav files as fallback to any filetype is allowed");
            return Ok(FileType::Wav);
        }
    } else if let Some((src_album, _src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        FileType::MP3,
    )) {
        println!("Found mp3 source album {src_album:?}");
        println!("NOT IMPLEMENTED: Album conversion mp3 => {dest_ft:?}");
        if allow_any {
            if !dest_artist_dir.exists() {
                let _ = std::fs::create_dir_all(&dest_artist_dir);
            }
            fs_extra::copy_items(&[&src_album.dir_path], &dest_artist_dir, &copy_options)?;
            println!("Copied over mp3 files as fallback to any filetype is allowed");
            return Ok(FileType::MP3);
        }
    }
    Err(anyhow!(
        "Failed to copy {:?} to {dest_artist_dir:?}",
        src_album.dir_path
    ))
}

fn convert_src_album(src: &Path, src_album: &Album, dest_ft: &FileType) -> Result<Album> {
    let desired_ft = dest_ft.to_possible_value().expect("");
    let desired_ft = desired_ft.get_name();
    // create album dir
    let src_artist_dir = src.join(&src_album.artist);
    if !src_artist_dir.exists() {
        let _ = std::fs::create_dir(&src_artist_dir);
    }

    let new_src_album_dir = src_artist_dir.join(format!("{} [{}]", src_album.title, &desired_ft));
    // copy over cover files
    src_album.cover_files.iter().for_each(|cf| {
        let cf_name = cf.file_name().expect("cover files muts have a file name!");
        let cf_dest = new_src_album_dir.join(cf_name);
        println!("COPY: {cf:?} -> {cf_dest:?}");
        let r = std::fs::copy(cf, src_artist_dir.join(cf_name));
        if r.is_err() {
            println!("copy failed!");
        }
    });
    // create converted files in correct dir
    if !new_src_album_dir.exists() {
        let _ = std::fs::create_dir(&new_src_album_dir);
    }
    let mut new_tracks = vec![];
    src_album.tracks.iter().for_each(|t| {
        let full_path = src_album.dir_path.join(t);
        let t_new = t.replace(".flac", &format!(".{desired_ft}"));
        let dst_path = new_src_album_dir.join(&t_new);
        println!("Track: {full_path:?} --> {dst_path:?}");
        match dest_ft {
            FileType::MP3 => {
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
            }
            _ => {
                println!(
                    "TODO: implement conversion {:?} --> {dest_ft:?}",
                    src_album.file_type()
                );
            }
        }
    });
    if new_tracks.len() == src_album.tracks.len() {
        Ok(Album::new(
            src_album.title.clone(),
            src_album.artist.clone(),
            new_tracks,
            new_src_album_dir,
            src_album.cover_files.clone(),
        ))
    } else {
        bail!("Failed to convert src album: {src_album:?} -->{new_src_album_dir:?} ");
    }
}

fn adb_copy_album(src_album: &Album, device: &mut ADBServerDevice) {
    // check whether artist dir exists on device
    let adb_artist_dir = format!("/storage/emulated/0/Music/{}", src_album.artist);
    if !dir_exists_on_adb_device(device, &adb_artist_dir) {
        let mut buf = BufWriter::new(Vec::new());
        let adb_dir_s = format!("\"{adb_artist_dir}\"");
        let command = vec!["mkdir", &adb_dir_s];
        let _ = device.shell_command(&command, &mut buf);
    }
    let adb_album_dir = format!("{}/{}", adb_artist_dir, src_album.title_without_filetype());
    if !dir_exists_on_adb_device(device, &adb_album_dir) {
        let mut buf = BufWriter::new(Vec::new());
        let adb_dir_s = format!("\"{adb_album_dir}\"");
        let command = vec!["mkdir", &adb_dir_s];
        let _ = device.shell_command(&command, &mut buf);
    }
    src_album.cover_files.iter().for_each(|cf| {
        let full_cover_file = src_album.dir_path.join(cf);
        let mut input = File::open(full_cover_file).expect("Cannot open file");
        let name = cf
            .file_name()
            .expect("Cover files must have a file name!")
            .to_str()
            .expect("Cover file name must be convertible to str");
        let full_cover_dst = format!("{adb_album_dir}/{name}");
        let _ = device.push(&mut input, &full_cover_dst);
    });
    src_album.tracks.iter().for_each(|tf| {
        let full_track_file = src_album.dir_path.join(tf);
        let mut input = File::open(full_track_file).expect("Cannot open track file");
        let full_track_dst = format!("{adb_album_dir}/{tf}");
        let _ = device.push(&mut input, &full_track_dst);
    });
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
