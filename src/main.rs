use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt::format,
    fs::{File, read_dir},
    io::BufWriter,
    path::{Component, Path, PathBuf},
    process::Command,
    str::FromStr,
};

use adb_client::{ADBDeviceExt, ADBServer, ADBServerDevice};
use clap::{Parser, Subcommand, ValueEnum};
use pathdiff::diff_paths;

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
    },
    /// add a directory to the destination list
    AddDest {
        #[arg(short, long)]
        directory: PathBuf,
        ft: FileType,
    },
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Serialize, Deserialize)]
enum FileType {
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
    destinations: Vec<(Destination, FileType)>,
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

#[derive(Clone, Debug)]
struct Album {
    title: String,
    artist: String,
    tracks: Vec<String>,
    dir_path: PathBuf,
    cover_files: Vec<PathBuf>,
}

impl Album {
    fn new(
        title: String,
        artist: String,
        tracks: Vec<String>,
        dir_path: PathBuf,
        cover_files: Vec<PathBuf>,
    ) -> Self {
        Album {
            title,
            artist,
            tracks,
            dir_path,
            cover_files,
        }
    }
    fn title_without_filetype(&self) -> String {
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

    fn file_type(&self) -> Option<FileType> {
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
        Commands::Config { subcommand } => {
            use ConfigCommands::*;
            let mut config = DirConfig::read()?;
            match subcommand {
                AddDest { directory, ft } => {
                    config
                        .destinations
                        .push((Destination::PathDest(directory), ft));
                }
                AddSource { directory } => config.source_directories.push(directory),
                AddADB { ft } => config.destinations.push((Destination::ADBDest, ft)),
            }
            config.write()?;
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
            let album_lookup = create_source_album_lookup(&config.source_directories);
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
                .for_each(|(dest, ft)| match dest {
                    Destination::PathDest(p) => {
                        let albums = albums_in_dir(p);
                        albums.iter().for_each(|a| {
                            if let Some(aft) = a.file_type() &&
                                 aft != *ft {
                                      println!("Found {a:?} with wrong filetype (is {aft:?}, but should be {ft:?})");                     
                            }
                        });
                        todo!()
                    }
                    Destination::ADBDest => {
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
                            .map(|l| {
                                PathBuf::from_str(l).expect("each line should be a valid path!")
                            })
                            .collect();
                        let pb = PathBuf::from_str("/storage/emulated/0/Music").unwrap();
                        let albums = group_files_into_albums(&music_paths, pb.as_path());
                        let mut albums_on_device = HashSet::new();


                        let album_lookup = create_source_album_lookup(&config.source_directories);

                        albums.iter().for_each(|a| {
                            if let Some(aft) = a.file_type(){
                              if aft != *ft {
                                  println!("Found {a:?} with wrong filetype (is {aft:?}, but should be {ft:?})");
if let Some((src_album, _src))= album_lookup.get(&(a.title.clone(), a.artist.clone(), ft.clone())){
    println!("Found source album {src_album:?}");
    println!("Will attempt to delete album on adb device at {:?}", a.dir_path);
    println!("Deleting {:?} on ADB device!", a.dir_path);
    del_album_on_device(a, &mut device);
    println!("copying {:?} to {:?} on ADB device!",src_album.dir_path, a.dir_path);
    adb_copy_album(src_album, &mut device);
albums_on_device.insert((a.title.clone(),a.artist.clone(), ft.clone()));

}else if let Some((src_album, src))=album_lookup.get(&(a.title.clone(),a.artist.clone(), FileType::Flac)){
println!("Found Flac source album {src_album:?}");
let _ = convert_src_album(src, src_album, ft);

println!("Deleting {:?} on ADB device!", a.dir_path);
del_album_on_device(a, &mut device);
println!("copying {:?} to {:?} on ADB device!",src_album.dir_path, a.dir_path);
adb_copy_album(src_album, &mut device);
albums_on_device.insert((a.title.clone(),a.artist.clone(), ft.clone()));
}
else if let Some((src_album, _src))=album_lookup.get(&(a.title.clone(),a.artist.clone(), FileType::Wav)){
println!("Found wav source album {src_album:?}");
println!("NOT IMPLEMENTED: Album conversion wav => mp3");
}
                            }
else {
albums_on_device.insert((a.title.clone(),a.artist.clone(), ft.clone()));
                              }
                            }
                            if a.file_type().is_none(){
println!("Failed to determine file type for {a:?}");
                            }
                        });

let album_lookup = create_source_album_lookup(&config.source_directories);
album_lookup.iter().for_each(|((album_title,album_artist, _), (album, _))|{
if !albums_on_device.iter().any(|(at, aa,_)|{at == album_title&& *aa == *album.artist}){
    ensure_album_is_on_device(album, ft, & album_lookup, &mut device);
albums_on_device.insert((album_title.to_string(),album_artist.to_string(), ft.clone()));
}
});
                    }
                });
            Ok(())
        }
    }
}

/// simply copies the album files to the device in the desired file type
/// does NOT delete any files on the device
fn ensure_album_is_on_device(
    src_album: &Album,
    dest_ft: &FileType,
    album_lookup: &HashMap<(String, String, FileType), (Album, PathBuf)>,
    device: &mut ADBServerDevice,
) -> bool {
    if let Some((src_album, _src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        dest_ft.clone(),
    )) {
        println!("Found source album {src_album:?}");
        adb_copy_album(src_album, device);
        return true;
    } else if let Some((src_album, src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        FileType::Flac,
    )) {
        println!("Found Flac source album {src_album:?}");
        if convert_src_album(src, src_album, dest_ft).is_ok() {
            adb_copy_album(src_album, device);
            return true;
        }
    } else if let Some((src_album, _src)) = album_lookup.get(&(
        src_album.title_without_filetype(),
        src_album.artist.clone(),
        FileType::Wav,
    )) {
        println!("Found wav source album {src_album:?}");
        println!("NOT IMPLEMENTED: Album conversion wav => mp3");
    }
    false
}

fn convert_src_album(src: &Path, src_album: &Album, dest_ft: &FileType) -> Result<Album> {
    let desired_ft = dest_ft.to_possible_value().expect("");
    let desired_ft = desired_ft.get_name();
    // create album dir
    let src_artist_dir = src.join(&src_album.artist);
    if !src_artist_dir.exists() {
        let _ = std::fs::create_dir(&src_artist_dir);
    }

    // copy over cover files
    src_album.cover_files.iter().for_each(|cf| {
        let cf_name = cf.file_name().expect("cover files muts have a file name!");
        let cf_dest = src_artist_dir.join(cf_name);
        println!("COPY: {cf:?} -> {cf_dest:?}");
        let r = std::fs::copy(cf, src_artist_dir.join(cf_name));
        if r.is_err() {
            println!("copy failed!");
        }
    });
    // create converted files in correct dir
    let new_src_album_dir = src_artist_dir.join(format!("{} [{}]", src_album.title, &desired_ft));
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

fn create_source_album_lookup(
    source_dirs: &[PathBuf],
) -> HashMap<(String, String, FileType), (Album, PathBuf)> {
    let mut album_lookup = HashMap::new();
    source_dirs.iter().for_each(|sd| {
        let albums = albums_in_dir(sd);
        albums.into_iter().for_each(|a| {
            if let Some(ft) = a.file_type() {
                album_lookup.insert(
                    (a.title_without_filetype(), a.artist.clone(), ft),
                    (a.clone(), sd.clone()),
                );
            }
        })
    });
    album_lookup
}

fn group_files_into_albums(file_paths: &[PathBuf], root: &Path) -> Vec<Album> {
    let mut album_lookup: HashMap<PathBuf, Album> = HashMap::new();
    file_paths.iter().for_each(|mp| {
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

    let mut album = album.trim().to_string();
    if let Some(ext) = MUSIC_EXTENSIONS
        .iter()
        .flat_map(|ext| [ext.to_string(), ext.to_uppercase()])
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
        artist,
        tracks,
        dir_path,
        cover_files,
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
