# eris
LRRbot's Discord part but in Rust.

## License
Licensed under Apache-2.0 ([LICENSE](LICENSE) or [https://www.apache.org/licenses/LICENSE-2.0](https://www.apache.org/licenses/LICENSE-2.0)).

## Setup instructions
Needs a [LRRbot](https://github.com/mrphlip/lrrbot) to run.

Roughly:
```bash
cd /path/to/lrrbot
# in a different terminal: . venv/bin/activate; python3 start_bot.py
cargo run --manifest-path /path/to/eris/Cargo.toml --release
```

