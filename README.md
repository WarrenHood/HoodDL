# HoodDL

[![Build](https://github.com/WarrenHood/HoodDL/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/WarrenHood/HoodDL/actions/workflows/build.yml)

A simple, blazingly fast download accelerator (written in Rust).

## Features

- Configurable number of connections to be used when downloading files.
- File is split into segments. All segments are downloaded concurrently (1 segment per connection)
- Progress bar for each segment/connection.
- Cancelled/failed downloads can be resumed if you specify the same number of connections as in the previous attempt
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
  -t, --target-total-chunk-size <TARGET_TOTAL_CHUNK_SIZE>
          The concurrent target total size of all chunks (MB) [default: 20]
      --chunk-retry-timeout <CHUNK_RETRY_TIMEOUT>
          Timeout in seconds before retrying a chunk download (don't set this too low) [default: 15]
      --chunk-retry-delay <CHUNK_RETRY_DELAY>
          Chunk retry delay in seconds [default: 5]
      --new-client-per-request
          Create a new client every time we make a request
  -h, --help
          Print help
  -V, --version
          Print version
```
