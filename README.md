# Music Organizer
Opinionated tool to organize music on your devices. You can setup a repository of music and copy it to different destinations in different file formats.
The tool uses discogs to complete music meta data.

## Features
- config via `toml`
- one source, multiple destinations (including an ADB device if it can be discovered)
- filetype preferences for each destination
- config manipulation via CLI
- music meta data completion via discogs (requires API setup)

## Assumptions
Your music source is setup like this:
```
root_dir
    |- Artist name
        |- album 1
            |- song1.mp3
            |- song2.mp3
            |- ...
            |- cover.png [optional]
        |- album 2
    |- Artist name - Album Name [MP3]
        |- song files (should be mp3)
        | cover.png [optional]
    |- Artist name - Album Name 
    
```
- One root directory
- the immediate child directories represent either
    - artists
        - In this case the directory should have the name of the artist (or a sanitized version). This name may be used to determine the meta data.
        - The children of the artist directory should represent albums (and should be named like that album)
    - albums. In this case the directory should have a name of the form `<Artist> - <Album name> [filetype]` or `<Artist> - <Album name>`.
- the album directories contain the song files and an optional image file that will be assumed to be the album cover.

## Usage
Use `cargo run -- --help` to get an overview of the available commands

## Build
`cargo build --release`
Copy the generated executable file whereever you prefer.
