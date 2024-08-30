# 各种相关组建、DEMO等等


### release

```bash
rustup target list --installed
# 不需要额外配置，貌似arm也可以允许x86的
rustup target add x86_64-apple-darwin   
cargo build -r --target x86_64-unknown-linux-musl
```

### windows

```bash
# 需要x86_linux 环境
cargo install cross 
cross build --target x86_64-pc-windows-gnu
```