fn main() -> std::io::Result<()> {
    // Bundle `VCRUNTIME140.DLL` on Windows:
    // #[cfg(all(windows))]
    // #[cfg(target_os = "windows")]
    // not ok. the meta also check the env
    static_vcruntime::metabuild();
    Ok(())
}