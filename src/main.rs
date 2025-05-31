use std::{collections::HashMap, io::BufWriter, path::PathBuf, str::FromStr};

use adb_client::{ADBDeviceExt, ADBServer};

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
}

fn main() {
    //    let vendor_id = 0x22d9;
    //  let product_id = 0x2765;
    //let mut device = ADBUSBDevice::new(vendor_id, product_id).expect("cannot find device");
    let mut server = ADBServer::default();
    let devices = server.devices();

    println!("devices: {devices:?}");
    let mut device = server.get_device().expect("cannot get device");
    device.list("/storage/emulated/0/Music/").unwrap();
    device.list("/storage").unwrap();
    device.list("Internalshared storage/Music").unwrap();
    let mut buf = BufWriter::new(Vec::new());
    let command = vec!["ls", "/storage/emulated/0/Music/"];
    let _ = device.shell_command(&command, &mut buf);
    // device.shell(&mut std::io::stdin(), Box::new(std::io::stdout()));
    let bytes = buf.into_inner().unwrap();
    let out = String::from_utf8_lossy(&bytes).to_string();
    println!("{out:?}");

    let mut buf = BufWriter::new(Vec::new());
    let command = vec!["find", "/storage/emulated/0/Music", "-type", "f"];
    let _ = device.shell_command(&command, &mut buf);
    // device.shell(&mut std::io::stdin(), Box::new(std::io::stdout()));
    let bytes = buf.into_inner().unwrap();
    let out = String::from_utf8_lossy(&bytes).to_string();
    let res: Vec<Vec<String>> = out
        .lines()
        .map(|l| {
            let rem = l.replace("/storage/emulated/0/Music/", "");
            let res: Vec<String> = rem.split('/').map(|s| s.to_string()).collect();
            res
        })
        .collect();
    let mut out = String::new();
    let mut album_to_files: HashMap<(String, String), Vec<String>> = HashMap::new();
    res.iter().for_each(|parts| {
        if parts.len() == 3 {
            let artist = parts[0].clone();
            let album = parts[1].clone();
            let track = parts[2].clone();
            if let Some(tracks) = album_to_files.get_mut(&(artist, album)) {
                tracks.push(track);
            } else {
                album_to_files.insert((parts[0].clone(), parts[1].clone()), vec![track]);
            }
        }
    });

    res.iter().for_each(|parts| {
        let tmp = parts.join(", ");
        out.push_str(&tmp);
        out.push('\n');
    });
    println!("{out}");

    let albums: Vec<Album> = album_to_files
        .into_iter()
        .map(|((artist, album), files)| {
            let dir_path = format!("/storage/emulated/0/Music/{artist}/{album}");
            let mut cover_file: Option<PathBuf> = None;
            let mut tracks = vec![];
            files.into_iter().for_each(|f| {
                if [".jpg"].iter().any(|ext| f.ends_with(ext)) {
                    if cover_file.is_none() {
                        cover_file = Some(PathBuf::from_str(&f).expect("Should be a valid path!"));
                    }
                } else if [".mp3", ".wav", ".flac"].iter().any(|ext| f.ends_with(ext)) {
                    tracks.push(f);
                }
            });
            Album::new(
                album,
                artist,
                tracks,
                PathBuf::from_str(&dir_path).expect("Should be a valid path!"),
                cover_file,
            )
        })
        .collect();
    albums.iter().for_each(|a| println!("{a:?}"));
}
