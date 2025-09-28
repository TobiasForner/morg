use std::{fs::File, io::BufWriter, path::PathBuf, str::FromStr};

use crate::{
    Album,
    album::{albums_in_dir, group_files_into_albums},
    del_album_on_device, dir_exists_on_adb_device,
};
use adb_client::{ADBDeviceExt, ADBServer, ADBServerDevice};
use anyhow::{Context, Result, bail};
use fs_extra::dir::CopyOptions;

pub trait Location {
    fn albums(&mut self) -> Result<Vec<Album>>;
    fn copy_full_album(&mut self, src_album: &Album) -> Result<()>;
    fn del_album(&mut self, album: &Album) -> Result<()>;
    fn copy_missing_files(&mut self, src_album: &Album, dst_album: &Album);

    fn to_string(&self) -> String;
}

#[derive(Debug)]
pub struct DirLocation {
    dir: PathBuf,
}
impl DirLocation {
    pub fn new(dir: PathBuf) -> Self {
        DirLocation { dir }
    }
}

impl Location for DirLocation {
    fn albums(&mut self) -> Result<Vec<Album>> {
        Ok(albums_in_dir(&self.dir))
    }

    fn copy_full_album(&mut self, src_album: &Album) -> Result<()> {
        let dst_path = self.dir.join(&src_album.parsed_artist);
        if !dst_path.exists() {
            std::fs::create_dir_all(&dst_path)?;
        }
        let copy_options = CopyOptions::new();
        println!("Copying {:?} to {dst_path:?}", src_album.dir_path);
        match fs_extra::copy_items(&[&src_album.dir_path], dst_path, &copy_options) {
            Ok(_) => Ok(()),
            Err(e) => bail!("Failed to copy items: {e:?}"),
        }
    }
    fn del_album(&mut self, album: &Album) -> Result<()> {
        std::fs::remove_dir_all(&album.dir_path)
            .context(format!("Failed to delete {}", album.overview()))
    }
    fn copy_missing_files(&mut self, src_album: &Album, dst_album: &Album) {
        println!("Copying missing files for {}", src_album.overview());
        if dst_album.dir_path.exists() {
            src_album.tracks.iter().for_each(|src_track| {
                if !dst_album.tracks.iter().any(|t| t == src_track) {
                    let dest = dst_album.dir_path.join(src_track);
                    let src_track = src_album.dir_path.join(src_track);
                    if src_track == dest {
                        println!("Did not find better src for {src_track:?}. Skipping.");
                    } else {
                        println!("Copying missing track {src_track:?} to {dest:?}");
                        let succ = std::fs::copy(src_track, dest);
                        if succ.is_err() {
                            println!("Something went wrong: {succ:?}");
                        }
                    }
                }
            });
            src_album.cover_files.iter().for_each(|src_cover| {
                if !src_album.cover_files.iter().any(|c| c == src_cover) {
                    let src_cover = src_album.dir_path.join(src_cover);
                    println!(
                        "Copying missing track {src_cover:?} to {:?}",
                        dst_album.dir_path
                    );
                    let succ = std::fs::copy(src_cover, &dst_album.dir_path);
                    if succ.is_err() {
                        println!("Something went wrong: {succ:?}");
                    }
                }
            });
        } else {
            /*println!(
                "copying {:?} to {:?}!",
                src_album.dir_path, dst_album.dir_path
            );*/
            let _ = self.copy_full_album(src_album);
        }
    }

    fn to_string(&self) -> String {
        format!("DirLocation({:?})", self.dir)
    }
}

#[derive(Debug)]
pub struct AdbLocation {
    device: ADBServerDevice,
}
impl AdbLocation {
    pub fn new() -> Result<Self> {
        let mut server = ADBServer::default();
        let devices = server.devices()?;
        if devices.len() != 1 {
            bail!("More than one adb device is connected: {devices:?}");
        } else {
            let device = &devices[0];
            println!("Found adb device with state {}", device.state);
        }

        println!("devices: {devices:?}");
        let Ok(device) = server.get_device() else {
            bail!("Failed to get ADB device!");
        };
        Ok(AdbLocation { device })
    }
}

impl Location for AdbLocation {
    fn albums(&mut self) -> Result<Vec<Album>> {
        let mut buf = BufWriter::new(Vec::new());
        let command = vec!["find", "/storage/emulated/0/Music", "-type", "f"];
        let _ = self.device.shell_command(&command, &mut buf);
        let bytes = buf.into_inner()?;
        let out = String::from_utf8_lossy(&bytes).to_string();
        let music_paths: Vec<PathBuf> = out
            .lines()
            .map(|l| PathBuf::from_str(l).expect("each line should be a valid path!"))
            .collect();
        let pb: PathBuf = PathBuf::from_str("/storage/emulated/0/Music")?;
        let albums = group_files_into_albums(&music_paths, pb.as_path());
        Ok(albums)
    }

    fn copy_full_album(&mut self, src_album: &Album) -> Result<()> {
        let adb_artist_dir = format!("/storage/emulated/0/Music/{}", &src_album.parsed_artist);
        if !dir_exists_on_adb_device(&mut self.device, &adb_artist_dir) {
            let mut buf = BufWriter::new(Vec::new());
            let adb_dir_s = format!("\"{adb_artist_dir}\"");
            let command = vec!["mkdir", &adb_dir_s];
            let _ = self.device.shell_command(&command, &mut buf);
        }
        let adb_album_dir =
            src_album.album_dir_with_ft(PathBuf::from("/storage/emulated/0/Music"), &None);
        let adb_album_dir = adb_album_dir.to_str().unwrap();
        let adb_album_dir = adb_album_dir.replace("\\", "/");
        let adb_album_dir_s = format!("\"{adb_album_dir}\"");
        if !dir_exists_on_adb_device(&mut self.device, &adb_album_dir_s) {
            let mut buf = BufWriter::new(Vec::new());
            // TODO: only replace unescaped double backslash
            let command = vec!["mkdir", &adb_album_dir_s];
            let success = self.device.shell_command(&command, &mut buf);
            if success.is_err() {
                println!("{success:?}");
            }
        }
        src_album.cover_files.iter().for_each(|cf| {
            let mut input = File::open(cf).expect("Cannot open file {cf:?}");
            let name = cf
                .file_name()
                .expect("Cover files must have a file name!")
                .to_str()
                .expect("Cover file name must be convertible to str")
                .replace(".jpeg", ".jpg");
            let full_cover_dst = format!("{adb_album_dir}/{name}");
            let _ = self.device.push(&mut input, &full_cover_dst);
        });
        src_album.tracks.iter().for_each(|tf| {
            let full_track_file = src_album.dir_path.join(tf);
            let input = File::open(&full_track_file);
            match input {
                Ok(mut input) => {
                    let full_track_dst = format!("{adb_album_dir}/{tf}");
                    let success = self.device.push(&mut input, &full_track_dst);
                    if success.is_err() {
                        println!("{success:?}");
                    }
                }
                Err(e) => println!("Cannot open track file {full_track_file:?}: {e:?}"),
            }
        });
        Ok(())
    }

    fn del_album(&mut self, album: &Album) -> Result<()> {
        del_album_on_device(album, &mut self.device);
        Ok(())
    }

    fn copy_missing_files(&mut self, src_album: &Album, dst_album: &Album) {
        let dst_dir = dst_album.dir_path.to_str().unwrap();
        if dir_exists_on_adb_device(&mut self.device, dst_dir) {
            src_album.tracks.iter().for_each(|src_track| {
                if !dst_album.tracks.iter().any(|t| t == src_track) {
                    let src_track = src_album.dir_path.join(src_track);
                    println!(
                        "Copying missing track {src_track:?} to {:?}",
                        dst_album.dir_path
                    );
                    let mut input = File::open(&src_track).expect("Cannot open file");
                    let name = src_track
                        .file_name()
                        .expect("Track files must have a file name!")
                        .to_str()
                        .expect("Cover file name must be convertible to str");
                    let full_track_dst = format!("{dst_dir}/{name}");
                    println!("PUSH {src_track:?} -> {full_track_dst}");
                    let success = self.device.push(&mut input, &full_track_dst);
                    if success.is_err() {
                        println!("{success:?}");
                    }
                }
            });
            src_album.cover_files.iter().for_each(|src_cover| {
                if !src_album.cover_files.iter().any(|c| c == src_cover) {
                    let src_cover = src_album.dir_path.join(src_cover);
                    println!(
                        "Copying missing cover file {src_cover:?} to {:?}",
                        dst_album.dir_path
                    );
                    let mut input = File::open(&src_cover)
                        .unwrap_or_else(|e| panic!("Cannot open file {src_cover:?}: {e}"));

                    let name = src_cover
                        .file_name()
                        .expect("Cover files must have a file name!")
                        .to_str()
                        .expect("Cover file name must be convertible to str")
                        .replace(".jpeg", ".jpg");
                    let full_cover_dst = format!("{dst_dir}/{name}");
                    let _ = self.device.push(&mut input, &full_cover_dst);
                }
            });
        } else {
            println!(
                "{:?} does not exist on device. Copying everything from {:?}!",
                dst_dir, src_album.dir_path,
            );
            let _ = self.copy_full_album(src_album);
        }
    }
    fn to_string(&self) -> String {
        "AdbLocation".to_string()
    }
}
