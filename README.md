# HoodDL

A simple, blazingly fast download accelerator (written in Rust).

## Features

- Configurable number of connections to be used when downloading files.
- File is split into segments. All segments are downloaded concurrently (1 segment per connection)
- Cancelled/failed downloads can be resumed if you specify the same number of connections as in the previous attempt.
- Setting cookies
- Configurable download chunk size in bytes for each connection (defaults to 8388608 bytes ie. 8MB)

Usage:

```bash
Usage: hooddl [OPTIONS] --url <URL>

Options:
  -u, --url <URL>
          Url of the file to download
  -f, --filename <FILENAME>
          Output file name
  -n, --num-connections <NUM_CONNECTIONS>
          Number of connections to use [default: 8]
  -c, --cookies <COOKIES>
          Cookies to use with the download requests
  -s, --segment-chunk-size <SEGMENT_CHUNK_SIZE>
          Size of chunk (in bytes) to download at a time per connection [default: 8388608]
  -h, --help
          Print help
  -V, --version
          Print version
```
