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
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin  
cargo install --force --locked bindgen-cli      
#   Consider installing the bindgen-cli: `cargo install --force --locked bindgen-cli`
#   See our User Guide for more information about bindgen:https://aws.github.io/aws-lc-rs/index.html
brew install cmake nasm ninja llvm
# ninja is required to enable CMake support.
cargo xwin build -r --target x86_64-pc-windows-msvc
```

* 或者用docker模式
* cross-rs/gnu