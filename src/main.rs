use anyhow::{Context, Result, bail};
use std::{
    collections::{HashMap, HashSet},
    fs::read_dir,
    io::BufWriter,
    path::{Component, Path, PathBuf},
    str::FromStr,
};

use adb_client::{ADBDeviceExt, ADBServer};
use clap::{Parser, Subcommand};
use pathdiff::diff_paths;

const IMAGE_EXTENSIONS: [&str; 2] = ["jpg", "png"];
const MUSIC_EXTENSIONS: [&str; 3] = ["mp3", "flac", "wav"];

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long, value_name = "PATH")]
    path: PathBuf,
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
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// add a directory to the sources list
    AddSource {
        #[arg(short, long)]
        directory: PathBuf,
    },
    /// add a directory to the destination list
    AddDest {
        #[arg(short, long)]
        directory: PathBuf,
        //TODO: specify filetype preferences
    },
}

struct DirConfig {
    source_directories: Vec<PathBuf>,
    destination_directories: Vec<PathBuf>,
}

impl DirConfig {
    fn read() -> Result<Self> {
        todo!("read config from file")
    }
}

#[derive(Debug)]
struct Album {
    title: String,
    artist: String,
    tracks: Vec<String>,
    dir_path: PathBuf,
    cover_file: Option<PathBuf>,
}

impl Album {
    fn new(
        title: String,
        artist: String,
        tracks: Vec<String>,
        dir_path: PathBuf,
        cover_file: Option<PathBuf>,
    ) -> Self {
        Album {
            title,
            artist,
            tracks,
            dir_path,
            cover_file,
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
            let cover_files: Vec<PathBuf> = self
                .cover_file
                .iter()
                .chain(other.cover_file.iter())
                .cloned()
                .collect();
            let cover_file = if cover_files.is_empty() {
                None
            } else if cover_files.len() == 1 || cover_files[0] == cover_files[1] {
                Some(cover_files[0].clone())
            } else {
                bail!("Failed to merge {self:?} and {other:?}!");
            };
            Ok(Album::new(
                self.title.clone(),
                self.artist.clone(),
                tracks,
                self.dir_path.clone(),
                cover_file,
            ))
        } else {
            bail!("Failed to merge {self:?} and {other:?}!")
        }
    }
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
        Commands::Config { subcommand: _ } => {
            let config = DirConfig::read()?;
            todo!("Config subcommands")
        }
        Commands::Sync => {
            let config = DirConfig::read();
            // TODO: sync using config
            let mut server = ADBServer::default();
            let devices = server.devices();

            println!("devices: {devices:?}");
            let mut device = server.get_device().expect("cannot get device");

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
            albums.iter().for_each(|a| println!("{a:?}"));

            println!("\n\n\n ==========");
            let albums = albums_in_dir(&args.path);
            albums.iter().for_each(|a| println!("{a:?}"));
            Ok(())
        }
    }
}

fn group_files_into_albums(file_paths: &[PathBuf], root: &Path) -> Vec<Album> {
    // TODO: handle potential trailing file type in directory name
    let mut album_lookup: HashMap<(String, String), Album> = HashMap::new();
    file_paths.iter().for_each(|mp| {
        let album = path_to_details(mp.into(), root.to_path_buf());
        if let Ok(album) = album {
            if let Some(a) = album_lookup.get(&(album.artist.clone(), album.title.clone())) {
                let merged = album.merge_with(a);
                if let Ok(merged) = merged {
                    album_lookup.insert((album.artist.clone(), album.title.clone()), merged);
                } else {
                    println!("ERROR: {merged:?}");
                }
            } else {
                album_lookup.insert((album.artist.clone(), album.title.clone()), album);
            };
        }
    });
    album_lookup.into_values().collect()
}

fn path_to_details(path: PathBuf, root_dir: PathBuf) -> Result<Album> {
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
        let artist = parts[0].clone();
        let rest = parts[1].replace(&format!("{artist} - "), "");
        if let Some((album, track)) = rest.rsplit_once(" - ") {
            (artist, album.to_string(), track.to_string())
        } else {
            bail!("Expected ' - ' delimiter between album name and track, but got {parts:?}");
        }
    } else {
        bail!("Could not parse details from {path:?}!");
    };
    let cover_file = if is_image(&path) {
        Some(file.clone().into())
    } else {
        None
    };
    let mut tracks = vec![];
    if is_music(&path) {
        tracks.push(file);
    }
    let dir_path = path
        .parent()
        .context("file should have a parent!")?
        .to_path_buf();
    Ok(Album::new(album, artist, tracks, dir_path, cover_file))
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
    read_dir(root).expect("").for_each(|de| {
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

fn albums_in_dir(root: &Path) -> Vec<Album> {
    let files = files_in_dir(root);
    group_files_into_albums(&files, root)
}
