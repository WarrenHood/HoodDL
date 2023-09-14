# HoodDL

A simple download accelerator written in Rust. Downloads are accelerated by using multiple connections.

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
